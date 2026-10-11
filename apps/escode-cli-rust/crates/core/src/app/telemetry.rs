//! 实时会话遥测事实（TS bootstrap/escode-protocol-v4/conversation-telemetry-facts.ts →
//! `v4/telemetry/event` 通知）：桌面埋点与运行中统计消费。只带 id、计数与状态，不带 prompt、
//! 工具入参或 provider URL；每条事实按 shared `conversationTelemetryFactSchema` 的 strict 形状给出。
use super::{Engine, Event};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct State {
    seq: HashMap<String, u64>,
    first_chunks: HashSet<String>,
    /// `computer-use/operation-event` 的会话内序号（cua_events.rs）。
    pub(super) cua_seq: std::collections::HashMap<String, u64>,
    /// 本会话最近一次完成的模型请求（usage.delta 的身份）。
    completed: HashMap<String, Value>,
}

const USAGE_SOURCES: [&str; 3] = ["main_turn", "subagent", "workflow_child"];

impl Engine {
    pub(super) fn emit_fact(&mut self, id: &str, turn: Option<&str>, kind: &str, fields: Value) {
        let seq = self.telemetry.seq.entry(id.to_owned()).or_default();
        let mut fact = Map::new();
        fact.insert("version".into(), 1.into());
        fact.insert("eventId".into(), self.clock.id().into());
        fact.insert("eventSeq".into(), (*seq).into());
        *seq += 1;
        fact.insert("occurredAt".into(), self.clock.now().into());
        fact.insert("sessionId".into(), id.into());
        if let Some(turn) = turn {
            fact.insert("turnId".into(), turn.into());
            if let Some(input) = self.turn_input(id, turn) {
                fact.insert("sourceCommandId".into(), input.into());
            }
        }
        fact.insert("kind".into(), kind.into());
        if let Some(fields) = fields.as_object() {
            fact.extend(
                fields
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        let fact = Value::Object(fact);
        self.local_ttft_fact(&fact);
        self.cua_from_fact(&fact);
        self.outbox.push(json!({ "method": "v4/telemetry/event", "params": fact }));
    }

    /// 本轮的输入 id（TS admission 的 inputId = 发起命令 id；用户轮与后台唤醒轮都有）。
    fn turn_input(&self, id: &str, turn: &str) -> Option<String> {
        let session = self.sessions.get(id)?;
        let command = session.rows.iter().find(|r| {
            r["turnId"] == turn && matches!(r["kind"].as_str(), Some("turnHeader" | "userInput"))
        });
        if let Some(command) = command.and_then(|r| r["sourceCommandId"].as_str()) {
            return Some(command.to_owned());
        }
        session
            .history
            .inputs
            .iter()
            .rev()
            .find(|i| i.turn == turn)
            .map(|i| i.entity.clone())
    }

    /// `turn.started`：回合起跑时（后台结果轮带 backgroundSource）。
    pub(super) fn telemetry_turn_started(&mut self, id: &str, turn: &str) {
        let source = self.sessions.get(id).and_then(|s| {
            s.rows
                .iter()
                .find(|r| r["turnId"] == turn && r["kind"] == "userInput")
                .and_then(|r| {
                    r["originMeta"]["backgroundSource"]
                        .as_str()
                        .map(str::to_owned)
                })
        });
        let source = source.filter(|s| matches!(s.as_str(), "bash" | "subagent" | "workflow"));
        self.emit_fact(
            id,
            Some(turn),
            "turn.started",
            json!({ "backgroundSource": source }),
        );
    }

    /// 运行事件 → 遥测事实（模型请求状态、流式块、工具生命周期、用量增量、回合终态）。
    pub(super) fn telemetry_event(&mut self, id: &str, turn: &str, event: &Event) {
        let now = self.clock.now();
        match event {
            Event::ModelStatus(status) => {
                let kind = status["type"].as_str().unwrap_or_default();
                if kind == "model_request_completed"
                    && USAGE_SOURCES
                        .contains(&status["querySource"].as_str().unwrap_or("main_turn"))
                {
                    self.telemetry
                        .completed
                        .insert(id.to_owned(), status.clone());
                }
                let mut fields = status.clone();
                if let Some(fields) = fields.as_object_mut() {
                    fields.remove("type");
                    fields.insert("status".into(), kind.into());
                }
                self.emit_fact(id, Some(turn), "model.request.status", fields);
            }
            Event::Text {
                text, reasoning, ..
            } => {
                // TS 的流键在一个回合内按通道复用（各步的同通道 part 共享键），首块每回合每通道一次。
                let channel = if *reasoning { "thought" } else { "text" };
                let first = self
                    .telemetry
                    .first_chunks
                    .insert(format!("{id}\0{turn}\0{channel}"));
                let length = text.encode_utf16().count();
                self.emit_fact(
                    id,
                    Some(turn),
                    "stream.chunk",
                    json!({ "channel": channel, "chunkLength": length, "firstChunk": first }),
                );
            }
            Event::ToolStart { call, .. } => {
                self.cua_tool_scheduled(id, turn, call);
                let fields =
                    json!({ "toolCallId": call["id"], "toolName": call["function"]["name"] });
                self.emit_fact(
                    id,
                    Some(turn),
                    "tool.lifecycle",
                    merge(&fields, json!({ "phase": "scheduled" })),
                );
                self.emit_fact(
                    id,
                    Some(turn),
                    "tool.lifecycle",
                    merge(&fields, json!({ "phase": "started" })),
                );
            }
            Event::ToolDone {
                id: call,
                tool,
                failed,
                ..
            } => {
                let started = self.sessions.get(id).and_then(|s| {
                    s.rows
                        .iter()
                        .rev()
                        .find(|r| r["kind"] == "toolCall" && r["toolCallId"] == call.as_str())
                        .and_then(|r| r["createdAt"].as_u64())
                });
                let fields = json!({
                    "phase": if *failed { "failed" } else { "completed" }, "toolCallId": call, "toolName": tool,
                    "durationMs": started.map(|s| now.saturating_sub(s)),
                });
                self.emit_fact(id, Some(turn), "tool.lifecycle", fields);
            }
            Event::ModelDone { usage, .. } => {
                let request = self.telemetry.completed.remove(id).unwrap_or(Value::Null);
                let input = int(&usage["prompt_tokens"]);
                let output = int(&usage["completion_tokens"]);
                let fields = json!({
                    "requestId": request["requestId"], "providerId": request["providerId"], "modelId": request["modelId"],
                    "inputTokens": input, "outputTokens": output,
                    "totalTokens": usage["total_tokens"].as_u64().unwrap_or(input + output),
                    "reasoningTokens": int(&usage["completion_tokens_details"]["reasoning_tokens"]),
                    "cacheReadTokens": int(&usage["prompt_tokens_details"]["cached_tokens"]),
                    "cacheWriteTokens": int(&usage["prompt_tokens_details"]["cache_write_tokens"]),
                });
                self.emit_fact(id, Some(turn), "usage.delta", fields);
            }
            Event::Finished {
                error, cancelled, ..
            } => {
                let cancelled =
                    *cancelled || self.active.get(id).is_some_and(|a| a.cancel.is_cancelled());
                let (status, result_type) = if cancelled {
                    ("interrupted", "cancelled")
                } else if error.is_some() {
                    ("failed", "error")
                } else {
                    ("success", "success")
                };
                let started = self.sessions.get(id).and_then(|s| {
                    s.rows
                        .iter()
                        .find(|r| r["kind"] == "turnHeader" && r["turnId"] == turn)
                        .and_then(|r| r["createdAt"].as_u64())
                });
                let tools = self.sessions.get(id).map_or(0, |s| {
                    s.rows
                        .iter()
                        .filter(|r| r["kind"] == "toolCall" && r["turnId"] == turn)
                        .count()
                });
                let mut fields = json!({
                    "status": status, "resultType": result_type,
                    "durationMs": started.map(|s| now.saturating_sub(s)), "toolCallCount": tools,
                });
                if cancelled {
                    fields["errorCode"] = "USER_INTERRUPT".into();
                    fields["errorMessage"] = "User stopped generation".into();
                } else if let Some(message) = error {
                    fields["errorCode"] = "fault.runtime.execution".into();
                    fields["errorMessage"] = message.clone().into();
                }
                self.emit_fact(id, Some(turn), "turn.terminal", fields);
                let prefix = format!("{id}\0{turn}\0");
                self.telemetry
                    .first_chunks
                    .retain(|k| !k.starts_with(&prefix));
            }
            _ => {}
        }
    }
}

fn int(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

fn merge(base: &Value, extra: Value) -> Value {
    let mut merged = base.clone();
    if let (Some(target), Some(fields)) = (merged.as_object_mut(), extra.as_object()) {
        target.extend(fields.clone());
    }
    merged
}

/// `permission.lifecycle` 的一条（TS PermissionRequested / PermissionResolved / PermissionDenied）。
pub(super) struct Permission<'a> {
    pub phase: &'a str,
    pub call_id: &'a str,
    pub tool: Option<&'a str>,
    pub request_id: Option<&'a str>,
    pub decision: Option<&'a str>,
}

impl Engine {
    pub(super) fn telemetry_permission(&mut self, id: &str, turn: Option<&str>, p: Permission<'_>) {
        let fields = json!({
            "phase": p.phase, "requestId": p.request_id, "toolCallId": p.call_id,
            "toolName": p.tool, "decision": p.decision,
        });
        self.emit_fact(id, turn, "permission.lifecycle", fields);
    }
}

impl Engine {
    /// `subagent.lifecycle`（TS SubagentSpawned / SubagentStopped）：父会话上的子代理诞生与结束。
    pub(super) fn telemetry_subagent(
        &mut self,
        parent: &str,
        phase: &str,
        task: &crate::domain::subagent::Task,
    ) {
        let turn = self.sessions.get(parent).and_then(|s| {
            s.rows
                .iter()
                .find(|r| r["toolCallId"] == task.call_id.as_str())
                .and_then(|r| r["turnId"].as_str().map(str::to_owned))
        });
        let stopped = phase == "stopped";
        let fields = json!({
            "phase": phase, "agentId": task.id, "agentType": task.agent_type, "childSessionId": task.child_id,
            "parentToolCallId": task.call_id, "background": task.background,
            "status": task.status,
            "errorMessage": if stopped && task.status == "failed" && !task.output.is_empty() {
                Value::from(task.output.clone())
            } else {
                Value::Null
            },
        });
        self.emit_fact(parent, turn.as_deref(), "subagent.lifecycle", fields);
    }
}
