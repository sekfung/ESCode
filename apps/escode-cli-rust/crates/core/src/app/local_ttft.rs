//! 本地首 token 时延观测（TS bootstrap/escode-protocol-v4/local-ttft.ts `LocalTtftRecorder`）：命令信封带
//! `ttft {version, observationId}` 时启用，跟随遥测事实记录接收 / 准入 / 起跑 / 首请求 / 首输出 / 终态，
//! 每次变化经 `v4/telemetry/local-ttft` 发出一份检查点。无业务裁决权，任何失败都不影响命令。
use super::Engine;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const MAX_PENDING: usize = 128;
const MAX_DETAILS: usize = 64;
const TTL_MS: f64 = 300_000.0;

pub(super) struct Recorder {
    instance_id: String,
    pub(super) records: Vec<(String, Value)>,
    pub(super) completed: Vec<(String, Value)>,
    /// commandId → "response" | "preparation" | "failed"（TS requestState）。
    request_state: HashMap<String, &'static str>,
}

impl Recorder {
    /// 进程实例 id（TS `instanceId = randomUUID()`）：时钟校准与观测记录按它对齐。
    pub(super) fn new(instance_id: String) -> Self {
        Self {
            instance_id,
            records: vec![],
            completed: vec![],
            request_state: HashMap::new(),
        }
    }
}

/// TS `localTtftNow`：epoch 毫秒（带小数）。
pub(super) fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

impl Engine {
    /// `v4/commands/query {clock: true}`：纯时钟探测（渲染端校准两侧时钟偏移）。
    pub(super) fn local_ttft_clock(
        &self,
        params: &Value,
        received_at: f64,
    ) -> anyhow::Result<Value> {
        let keys = params["commands"].as_array().map_or(&[][..], Vec::as_slice);
        anyhow::ensure!(
            !keys.is_empty() && keys.len() <= 64,
            "Invalid command query size"
        );
        // TS commandsQueryParamsSchema：时钟探测不能查询会话命令。
        anyhow::ensure!(
            keys.iter().all(|k| k["sessionId"].is_null()),
            "clock probes cannot query session commands"
        );
        let results: Vec<Value> = params["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|key| json!({ "key": key, "result": "unknown" }))
            .collect();
        Ok(json!({
            "results": results,
            "clock": { "instanceId": self.local_ttft.instance_id, "receivedAt": received_at, "sentAt": now() },
        }))
    }

    /// 命令到达：信封带 ttft 时登记；容量满时返回 false（ACK 带 `ttftExcluded: "capacity"`）。
    pub(super) fn local_ttft_receive(&mut self, params: &Value) -> bool {
        self.local_ttft_prune();
        let (Some(ttft), Some(command)) =
            (params["ttft"].as_object(), params["commandId"].as_str())
        else {
            return true;
        };
        let known = |list: &[(String, Value)]| list.iter().any(|(id, _)| id == command);
        if known(&self.local_ttft.records) || known(&self.local_ttft.completed) {
            return true;
        }
        if self.local_ttft.records.len() >= MAX_PENDING {
            return false;
        }
        let session = params["sessionId"].as_str();
        let busy = session
            .and_then(|s| self.sessions.get(s))
            .is_some_and(|s| s.running());
        let mut record = Map::new();
        record.insert(
            "version".into(),
            ttft.get("version").cloned().unwrap_or(1.into()),
        );
        record.insert(
            "observationId".into(),
            ttft.get("observationId").cloned().unwrap_or_default(),
        );
        record.insert(
            "instanceId".into(),
            self.local_ttft.instance_id.clone().into(),
        );
        record.insert("commandId".into(), command.into());
        if let Some(session) = session {
            record.insert("sessionId".into(), session.into());
        }
        record.insert("receivedAt".into(), now().into());
        record.insert(
            "sendMode".into(),
            if busy { "queued" } else { "idle" }.into(),
        );
        record.insert("details".into(), json!([]));
        self.local_ttft
            .records
            .push((command.to_owned(), Value::Object(record)));
        true
    }

    pub(super) fn local_ttft_admitted(&mut self, command: &str) {
        if let Some(index) = self
            .local_ttft
            .records
            .iter()
            .position(|(id, _)| id == command)
        {
            let record = &mut self.local_ttft.records[index].1;
            if record.get("admittedAt").is_none() {
                record["admittedAt"] = now().into();
            }
            self.local_ttft_checkpoint(index);
        }
    }

    /// 遥测事实 → 观测记录（TS LocalTtftRecorder.fact / output）。
    pub(super) fn local_ttft_fact(&mut self, fact: &Value) {
        if self.local_ttft.records.is_empty() {
            return;
        }
        let kind = fact["kind"].as_str().unwrap_or_default();
        // 首输出按会话与回合匹配（不依赖 sourceCommandId）。
        if kind == "stream.chunk" || (kind == "tool.lifecycle" && fact["phase"] == "scheduled") {
            let output = match (kind, fact["channel"].as_str()) {
                ("stream.chunk", Some("thought")) => "reasoning",
                ("stream.chunk", _) => "text",
                _ => "tool",
            };
            self.local_ttft_output(fact["sessionId"].as_str(), fact["turnId"].as_str(), output);
        }
        let Some(index) = fact["sourceCommandId"]
            .as_str()
            .and_then(|c| self.local_ttft.records.iter().position(|(id, _)| id == c))
        else {
            return;
        };
        let command = self.local_ttft.records[index].0.clone();
        let record = &mut self.local_ttft.records[index].1;
        if record
            .get("sessionId")
            .is_some_and(|s| *s != fact["sessionId"])
        {
            return;
        }
        let t = now();
        if kind == "turn.started" {
            record["sessionId"] = fact["sessionId"].clone();
            record["turnId"] = fact["turnId"].clone();
            if record.get("executionAt").is_none() {
                record["executionAt"] = t.into();
            }
            self.local_ttft_checkpoint(index);
            return;
        }
        if record.get("turnId").is_none() && kind == "turn.terminal" {
            record["turnId"] = fact["turnId"].clone();
        }
        if record["turnId"] != fact["turnId"] {
            return;
        }
        if kind == "model.request.status" && record.get("outputAt").is_none() {
            let source = fact["querySource"].as_str().unwrap_or("main_turn");
            if source == "main_turn" || source == "compact" {
                let role = if source == "main_turn" {
                    "response"
                } else {
                    "preparation"
                };
                let request = fact["requestId"].as_str().unwrap_or_default();
                let status = fact["status"].as_str().unwrap_or_default();
                let attempt_id = format!("attempt:{request}");
                let mut truncated = false;
                let details = record["details"].as_array_mut().unwrap();
                let exists = details.iter().any(|d| d["id"] == attempt_id.as_str());
                if status == "model_request_started" && !exists {
                    self.local_ttft.request_state.insert(command.clone(), role);
                    if details.len() < MAX_DETAILS {
                        details.push(json!({"id": attempt_id, "stage": "attempt", "start": t, "requestId": request, "role": role, "source": "cli"}));
                    } else {
                        truncated = true;
                    }
                    for wait in details.iter_mut() {
                        if wait["stage"] == "retry_wait"
                            && wait.get("end").is_none()
                            && wait["role"] == role
                        {
                            wait["end"] = t.into();
                            wait["outcome"] = "completed".into();
                        }
                    }
                    if role == "response" {
                        if record.get("requestAt").is_none() {
                            record["requestAt"] = t.into();
                        }
                        record["requestId"] = request.into();
                        record["model"] = fact["modelId"].clone();
                        record["provider"] = fact["providerId"].clone();
                    }
                }
                let details = record["details"].as_array_mut().unwrap();
                if matches!(status, "model_request_failed" | "model_request_completed")
                    && let Some(attempt) = details
                        .iter_mut()
                        .find(|d| d["id"] == attempt_id.as_str() && d.get("end").is_none())
                {
                    attempt["end"] = t.into();
                    attempt["outcome"] = if status == "model_request_failed" {
                        "failed"
                    } else {
                        "completed"
                    }
                    .into();
                    if role == "response"
                        && status == "model_request_failed"
                        && record["requestId"] == request
                    {
                        self.local_ttft
                            .request_state
                            .insert(command.clone(), "failed");
                    }
                }
                let details = record["details"].as_array_mut().unwrap();
                let retry_id = format!("retry:{request}");
                if status == "model_retry_scheduled"
                    && !details.iter().any(|d| d["id"] == retry_id.as_str())
                {
                    if details.len() < MAX_DETAILS {
                        details.push(json!({"id": retry_id, "stage": "retry_wait", "start": t, "requestId": request, "role": role, "source": "cli"}));
                    } else {
                        truncated = true;
                    }
                }
                if truncated {
                    record["truncated"] = true.into();
                }
            }
        }
        self.local_ttft_checkpoint(index);
        if kind == "turn.terminal" {
            let terminal = match fact["status"].as_str() {
                Some("success") => "completed",
                Some("failed") => "failed",
                _ => "cancelled",
            };
            self.local_ttft.records[index].1["terminal"] = terminal.into();
            self.local_ttft_checkpoint(index);
            self.local_ttft_retire(index);
        }
    }

    fn local_ttft_output(&mut self, session: Option<&str>, turn: Option<&str>, kind: &str) {
        let mut index = 0;
        while index < self.local_ttft.records.len() {
            let (command, record) = &mut self.local_ttft.records[index];
            let state = self.local_ttft.request_state.get(command.as_str()).copied();
            let matches = record["sessionId"].as_str() == session
                && record["turnId"].as_str() == turn
                && record.get("requestAt").is_some();
            if matches
                && !matches!(state, Some("failed" | "preparation"))
                && record.get("outputAt").is_none()
            {
                let t = now();
                record["outputAt"] = t.into();
                record["outputKind"] = kind.into();
                let attempt_id = format!(
                    "attempt:{}",
                    record["requestId"].as_str().unwrap_or_default()
                );
                if let Some(attempt) = record["details"].as_array_mut().and_then(|d| {
                    d.iter_mut()
                        .find(|d| d["id"] == attempt_id.as_str() && d.get("end").is_none())
                }) {
                    attempt["end"] = t.into();
                    attempt["outcome"] = "first_output".into();
                }
                // TS output() 不发检查点：首输出时刻只经增量帧附着送达渲染端。
            }
            if matches && kind == "text" {
                self.local_ttft_retire(index);
                continue;
            }
            index += 1;
        }
    }

    fn local_ttft_checkpoint(&mut self, index: usize) {
        let record = &mut self.local_ttft.records[index].1;
        let revision = record["revision"].as_u64().unwrap_or(0) + 1;
        record["revision"] = revision.into();
        let facts = record.clone();
        self.outbox
            .push(json!({ "method": "v4/telemetry/local-ttft", "params": facts }));
    }

    fn local_ttft_retire(&mut self, index: usize) {
        let (command, record) = self.local_ttft.records.remove(index);
        self.local_ttft.request_state.remove(&command);
        self.local_ttft.completed.push((command, record));
        if self.local_ttft.completed.len() > MAX_PENDING {
            self.local_ttft.completed.remove(0);
        }
    }

    fn local_ttft_prune(&mut self) {
        let t = now();
        let fresh = |(_, r): &(String, Value)| t - r["receivedAt"].as_f64().unwrap_or(t) <= TTL_MS;
        self.local_ttft.records.retain(fresh);
        self.local_ttft.completed.retain(fresh);
    }
}

