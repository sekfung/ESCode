//! `-p` 的一轮执行（docs/specs/rust-headless-prompt.md「执行」）：建 / 续会话 → 发送输入 → 自动拒绝审批 → 等终态 → 汇总。
use super::args::Parsed;
use super::client::Client;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

/// Node `-p` 的缺省模式。
const DEFAULT_MODE: &str = "yolo";
/// `v4/conversation/rowsRange` 的单页上限。
const ROW_PAGE_LIMIT: u64 = 200;
/// 本轮的终态。
const TERMINAL_PHASES: [&str; 3] = ["completedSuccess", "error", "completedInterrupted"];
const IMAGE_MIME: [(&str, &str); 5] = [
    ("gif", "image/gif"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
];
const VIDEO_MIME: [(&str, &str); 6] = [
    ("mp4", "video/mp4"),
    ("m4v", "video/x-m4v"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
    ("mkv", "video/x-matroska"),
    ("avi", "video/x-msvideo"),
];

/// 一轮的结果（输出层按格式渲染）。
pub struct Outcome {
    pub session_id: String,
    pub trace_id: Option<String>,
    pub turn_id: Option<String>,
    pub response: String,
    pub usage: Option<Value>,
    pub event_count: usize,
    pub phase: String,
    pub last_error: Option<String>,
    pub context_used: Value,
    pub context_window: Value,
}

/// Node `inferAttachmentTypeFromPath` 的 V4 形态：图片 / 视频 / PDF 按扩展名给 MIME，其余文件按文本。
fn mime_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    IMAGE_MIME
        .iter()
        .chain(VIDEO_MIME.iter())
        .find(|(e, _)| *e == ext)
        .map(|(_, m)| *m)
        .unwrap_or(if ext == "pdf" {
            "application/pdf"
        } else {
            "text/plain"
        })
}

/// `--attach`：相对路径按工作区解析，不存在的文件静默丢弃（Node 同样）。
async fn attachments(paths: &[String], cwd: &Path) -> Vec<Value> {
    let mut out = vec![];
    for raw in paths {
        let path = cwd.join(raw);
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(json!({
            "ref": path.to_string_lossy(),
            "fileName": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "mime": mime_of(&path),
            "bytes": meta.len(),
        }));
    }
    out
}

/// `-c`：本工作区最近更新的根会话（Node `resolveLatestSession`：roots、time_updated desc）。
async fn latest_session(client: &mut Client, workspace: &str) -> Result<String> {
    let listed = client
        .request(
            "session/list",
            json!({"workspace": {"workspacePath": workspace, "workspaceKey": workspace}}),
        )
        .await?;
    listed["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["parentSessionId"].is_null())
        .and_then(|s| s["sessionId"].as_str())
        .map(str::to_owned)
        .with_context(|| format!("No resumable session found for {workspace}"))
}

/// 自动结算交互：审批选拒绝（Node headless deny broker），提问回 decline。返回是否处理过。
async fn settle_interactions(
    client: &mut Client,
    session: &str,
    pending: &Value,
    handled: &mut HashSet<String>,
) -> Result<()> {
    for interaction in pending.as_array().into_iter().flatten() {
        let Some(id) = interaction["interactionId"].as_str() else {
            continue;
        };
        if !handled.insert(id.to_owned()) {
            continue;
        }
        let answer = if interaction["kind"] == "permission" {
            let deny = interaction["payload"]["options"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|o| o["kind"] == "deny" || o["optionId"] == "deny")
                .and_then(|o| o["optionId"].as_str())
                .unwrap_or("deny");
            json!({"optionId": deny})
        } else {
            json!({"action": "decline"})
        };
        client
            .command(
                Some(session),
                "resolveInteraction",
                json!({"interactionId": id, "answer": answer}),
            )
            .await?;
    }
    Ok(())
}

/// 本轮最后一个模型步骤的正文：本轮最后一个工具行之后的 assistantText（Node `result.response`）。
fn final_response(rows: &[Value], turn: &str) -> String {
    let turn_rows: Vec<&Value> = rows.iter().filter(|r| r["turnId"] == turn).collect();
    let start = turn_rows
        .iter()
        .rposition(|r| r["kind"] == "toolCall")
        .map_or(0, |i| i + 1);
    turn_rows[start..]
        .iter()
        .filter(|r| r["kind"] == "assistantText" && r["state"] != "interrupted")
        .filter_map(|r| r["text"].as_str())
        .collect()
}

/// 本轮用量：累加本会话本轮的 `usage.delta` 遥测（每次模型请求一条，Node 回合用量聚合同口径）；
/// WebFetch / WebSearch 次数取本轮的工具调用数。没有任何模型请求时为 None（Node 同样省略）。
#[derive(Default)]
struct TurnUsage {
    requests: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

impl TurnUsage {
    fn add(&mut self, event: &Value) {
        let n = |k: &str| event[k].as_u64().unwrap_or(0);
        self.requests += 1;
        self.input += n("inputTokens");
        self.output += n("outputTokens");
        self.cache_read += n("cacheReadTokens");
        self.cache_write += n("cacheWriteTokens");
        self.reasoning += n("reasoningTokens");
    }

    fn summary(&self, rows: &[Value], turn: Option<&str>) -> Option<Value> {
        if self.requests == 0 {
            return None;
        }
        let tool_calls = |name: &str| {
            rows.iter()
                .filter(|r| {
                    turn.is_some_and(|t| r["turnId"] == t)
                        && r["kind"] == "toolCall"
                        && r["toolName"] == name
                })
                .count()
        };
        Some(json!({
            "source": "provider",
            "modelRequestCount": self.requests,
            "inputTokens": self.input,
            "outputTokens": self.output,
            "totalTokens": self.input + self.output,
            "cacheReadTokens": self.cache_read,
            "cacheWriteTokens": self.cache_write,
            "reasoningTokens": self.reasoning,
            "webFetchRequests": tool_calls("WebFetch"),
            "webSearchRequests": tool_calls("WebSearch"),
        }))
    }
}

/// 本轮进度：帧增量（阶段、错误、待处理交互）与用量遥测。
struct Progress {
    started: bool,
    phase: String,
    event_count: usize,
    last_error: Option<String>,
    usage: TurnUsage,
}

impl Progress {
    fn new() -> Self {
        Self {
            started: false,
            phase: String::new(),
            event_count: 0,
            last_error: None,
            usage: TurnUsage::default(),
        }
    }

    fn done(&self) -> bool {
        self.started && TERMINAL_PHASES.contains(&self.phase.as_str())
    }

    /// 记录一条通知；返回其中出现的待处理交互（由调用方自动结算）。
    fn observe(&mut self, message: &Value, session: &str, topic: &str) -> Vec<Value> {
        let params = &message["params"];
        if message["method"] == "v4/telemetry/event" {
            if params["kind"] == "usage.delta" && params["sessionId"] == session {
                self.usage.add(params);
            }
            return vec![];
        }
        if message["method"] != "v4/conversation/frame" || params["topic"] != topic {
            return vec![];
        }
        let mut pending = vec![];
        for delta in params["frame"]["payload"]["deltas"]
            .as_array()
            .into_iter()
            .flatten()
        {
            self.event_count += 1;
            let patch = &delta["patch"];
            if let Some(next) = patch["control"]["phase"].as_str() {
                self.started |= next == "running";
                self.phase = next.to_owned();
            }
            if let Some(error) = patch["control"]["lastError"]["message"].as_str() {
                self.last_error = Some(error.to_owned());
            }
            if let Some(list) = patch["pendingInteractions"].as_array() {
                pending.extend(list.iter().cloned());
            }
        }
        pending
    }
}

pub async fn run_turn(
    client: &mut Client,
    args: &Parsed,
    workspace: &str,
    cwd: &Path,
) -> Result<Outcome> {
    let session = match (&args.resume, args.continue_session) {
        (Some(id), _) => id.clone(),
        (None, true) => latest_session(client, workspace).await?,
        (None, false) => client
            .command(None, "createSession", json!({"workspaceId": workspace}))
            .await?["result"]["sessionId"]
            .as_str()
            .context("createSession returned no session id")?
            .to_owned(),
    };
    let topic = format!("conversation/{session}");
    client
        .request(
            "v4/conversation/subscribe",
            json!({"topic": topic, "connectionId": "headless", "clientMode": "desktop-continuous"}),
        )
        .await?;
    // Node `DEFAULT_HEADLESS_PROMPT_MODE = "yolo"`：未给 --mode 时按 yolo，且总是覆盖会话已存的模式（含续接）。
    let mode = args.mode.clone().unwrap_or_else(|| DEFAULT_MODE.to_owned());
    let mut payload = json!({"text": args.prompt.clone().unwrap_or_default(), "mode": mode});
    let attached = attachments(&args.attach, cwd).await;
    if !attached.is_empty() {
        payload["attachments"] = attached.into();
    }
    if !args.disallowed_tools.is_empty() {
        payload["toolDisallowlist"] = args.disallowed_tools.clone().into();
    }
    client.command(Some(&session), "sendText", payload).await?;
    let mut progress = Progress::new();
    let mut handled = HashSet::new();
    while !progress.done() {
        let message = client.notification().await?;
        let pending = Value::Array(progress.observe(&message, &session, &topic));
        settle_interactions(client, &session, &pending, &mut handled).await?;
    }
    // 最新一页（上限 200 行）：本轮最终回答必然在尾部；轮首行可能不在页内，轮次 id 取最后一行的。
    let rows = client
        .request(
            "v4/conversation/rowsRange",
            json!({"sessionId": session, "limit": ROW_PAGE_LIMIT}),
        )
        .await?;
    let rows = rows["rows"].as_array().cloned().unwrap_or_default();
    let turn_id = rows
        .iter()
        .rev()
        .find_map(|r| r["turnId"].as_str())
        .map(str::to_owned);
    let read = client
        .request("session/read", json!({"sessionId": session}))
        .await?;
    let projection = &read["projection"];
    Ok(Outcome {
        response: turn_id
            .as_deref()
            .map(|t| final_response(&rows, t))
            .unwrap_or_default(),
        usage: progress.usage.summary(&rows, turn_id.as_deref()),
        session_id: session,
        trace_id: read["session"]["traceId"].as_str().map(str::to_owned),
        turn_id,
        event_count: progress.event_count,
        last_error: progress.last_error,
        phase: progress.phase,
        context_used: projection["contextUsed"].clone(),
        context_window: projection["contextWindow"].clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_is_the_final_step_text() {
        let rows = vec![
            json!({"turnId": "t", "kind": "assistantText", "text": "let me look. ", "state": "complete"}),
            json!({"turnId": "t", "kind": "toolCall"}),
            json!({"turnId": "t", "kind": "assistantText", "text": "answer", "state": "complete"}),
            json!({"turnId": "old", "kind": "assistantText", "text": "previous", "state": "complete"}),
        ];
        assert_eq!(final_response(&rows, "t"), "answer");
        assert_eq!(mime_of(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_of(Path::new("doc.pdf")), "application/pdf");
        assert_eq!(mime_of(Path::new("notes.md")), "text/plain");
    }

    #[test]
    fn turn_usage_sums_requests_and_counts_web_tools() {
        let mut usage = TurnUsage::default();
        usage.add(&json!({"inputTokens": 10, "outputTokens": 4}));
        usage.add(&json!({"inputTokens": 10, "outputTokens": 4, "cacheReadTokens": 2}));
        let rows = vec![
            json!({"turnId": "t", "kind": "toolCall", "toolName": "WebFetch"}),
            json!({"turnId": "x", "kind": "toolCall", "toolName": "WebFetch"}),
        ];
        let summary = usage.summary(&rows, Some("t")).unwrap();
        assert_eq!(summary["modelRequestCount"], 2);
        assert_eq!(summary["inputTokens"], 20);
        assert_eq!(summary["totalTokens"], 28);
        assert_eq!(summary["cacheReadTokens"], 2);
        assert_eq!(summary["webFetchRequests"], 1);
        assert!(TurnUsage::default().summary(&rows, Some("t")).is_none());
    }
}
