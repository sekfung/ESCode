//! OffPeak 工具（OffPeakCreate / OffPeakList）的校验、Host 请求、守卫与输出，逐条对齐 TS
//! `core/src/tool/handlers/off-peak.ts` 与 `bootstrap/src/escode-protocol/offpeak-port.ts`。
//! Host 调用由调用方注入（异步闭包）。见 docs/specs/rust-offpeak.md 第一期。

use std::future::Future;

use serde_json::{Value, json};

use crate::cron::HostError;
use crate::json_order::Json;

pub const TOOLS: [&str; 2] = ["OffPeakCreate", "OffPeakList"];
const FROM_OFF_PEAK_RUN: &str = "Cannot create an idle-time task while running an idle-time task.";
const IN_BOUND_SESSION: &str = "This session already has a pending idle-time task. Wait for it to finish (or cancel it in Automations) before creating another one here.";
const BOUND_CHECK_FAILED: &str =
    "Cannot verify whether this session already has a pending idle-time task; try again later.";
const TERMINAL: [&str; 3] = ["completed", "failed", "cancelled"];
const MODES: [&str; 4] = ["build", "edit", "plan", "yolo"];

/// 闲时派发轮额外隐藏的工具（TS `OFF_PEAK_MUTATION_TOOL_NAMES`，docs/specs/rust-offpeak.md 第二期）。
pub const MUTATION_TOOLS: [&str; 3] = ["OffPeakCreate", "SendMessage", "Workflow"];
pub const SEND_MESSAGE_HINT: &str =
    "Spawn a new foreground Agent with the full context instead of resuming a completed one.";
pub const BASH_BACKGROUND_DENIED: &str = "Idle-time tasks do not support background commands. Run this command in the foreground without run_in_background.";
/// core 传给 Bash 的内部参数：闲时受限轮关闭超时自动转后台（入参校验之后加入，不进入模型与行数据）。
pub const FOREGROUND_ONLY_ARG: &str = "__escodeOffPeakForegroundOnly";

/// TS `resolveTurnOffPeakTaskId`：显式 offPeakTaskId，否则取 `offpeak-` 前缀 inputId 的 `:` 之前部分。
pub fn turn_task_id(explicit: Option<&str>, input_id: &str) -> Option<String> {
    if let Some(id) = explicit.map(str::trim).filter(|id| !id.is_empty()) {
        return Some(id.to_owned());
    }
    let input = input_id.trim();
    if !input.starts_with("offpeak-") {
        return None;
    }
    let id = input.split(':').next().unwrap_or(input);
    (id.len() > "offpeak-".len()).then(|| id.to_owned())
}

/// 本轮与会话事实。
pub struct Turn {
    /// 受限轮（TS `isOffPeakCreateRestrictedTurn`）：handler 层拒绝 OffPeakCreate。
    pub off_peak_turn: bool,
    /// 本轮闲时派发身份（TS record.activeOffPeakTaskId）：端口层拒绝递归创建。
    pub active_off_peak_task_id: Option<String>,
    pub session_id: String,
}

/// 成功时为 (输出, `JSON.stringify(output)`)；失败时为模型可见错误文案。
pub type Outcome = Result<(Value, String), String>;

/// TS `assertNotOffPeakTurn`：闲时派发轮拒绝会落到用户套餐的工具。
pub fn off_peak_turn_denial(tool: &str, hint: Option<&str>) -> String {
    let hint = hint.map(|h| format!(" {h}")).unwrap_or_default();
    format!("{tool} is not allowed while running an idle-time task.{hint}")
}

/// TS `OffPeakCreateInputSchema`（strict、trim）。只剩 JSON Schema 查不到的问题：空白字符串。
fn parse_create(input: &Value) -> Result<Vec<(&'static str, String)>, String> {
    let mut fields = Vec::new();
    let mut issues = Vec::new();
    for key in ["title", "prompt", "permissionMode", "model", "thoughtLevel"] {
        let Some(value) = input.get(key).and_then(Value::as_str) else {
            continue;
        };
        let trimmed = crate::web_fetch::js_trim(value);
        if key == "permissionMode" {
            if MODES.contains(&value) {
                fields.push((key, value.to_owned()));
            }
        } else if trimmed.is_empty() {
            issues.push(json!({"code":"too_small","minimum":1,"type":"string","inclusive":true,"exact":false,"message":"String must contain at least 1 character(s)","path":[key]}));
        } else {
            fields.push((key, trimmed.to_owned()));
        }
    }
    if issues.is_empty() {
        return Ok(fields);
    }
    // zod issue 的键顺序：code, minimum, type, inclusive, exact, message, path（ZodError.message 即其 2 空格缩进 JSON）。
    let ordered: Vec<Json> = issues
        .iter()
        .map(|issue| {
            let mut object = Json::object();
            for key in ["code", "minimum", "type", "inclusive", "exact", "message", "path"] {
                object.set(key, Json::parse(&issue[key].to_string()).unwrap_or(Json::Null));
            }
            object
        })
        .collect();
    Err(Json::Array(ordered).pretty())
}

/// TS `toOffPeakTaskSummary`：只保留快照字段，按固定顺序、省略缺失的可选字段。
fn summary(task: &Json) -> Json {
    let mut out = Json::object();
    for key in ["offPeakTaskId", "title", "status", "queuePosition", "sessionId", "createdAt"] {
        if let Some(value) = task.get(key).filter(|v| !matches!(v, Json::Null)) {
            out.set(key, value.clone());
        }
    }
    out
}

fn finish(output: Json) -> Outcome {
    let text = output.compact();
    let data: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok((data, text))
}

/// TS `throwOffPeakCreateFailure`：按分类与错误码给出稳定文案，不按 message 猜测。
fn failure_detail(category: &str, code: &str) -> &'static str {
    match (category, code) {
        ("quota_3103", _) => "The idle-time task quota is used up for now. Tell the user the free quota is exhausted and they can retry later or review tasks in Automations.",
        ("eligibility_3101", _) => "The current account has no eligible Coding Plan connection for idle-time tasks. Tell the user to select a ZAI/BigModel Coding Plan connection first.",
        ("client_validation", "model_not_allowed") => "The requested model is not in the idle-time allowed model list. Omit the model field to use the default allowed model.",
        ("client_validation", "session_bound") => "This session already has a pending idle-time task. Tell the user to wait for it to finish or cancel it in Automations before creating another one here.",
        ("client_validation", "offpeak_disabled") => "Idle-time tasks are not enabled for this account right now. Tell the user the feature is unavailable; do not retry with different parameters.",
        ("client_validation", _) => "The idle-time task input was rejected by validation.",
        ("network", _) => "The idle-time ticket service is unreachable. Tell the user to retry later.",
        _ => "Creating the idle-time task failed. Tell the user to retry from the Automations page.",
    }
}

/// TS handler + protocol port 主流程。`call(method, params)` 返回 Host 结果（保持键顺序的 JSON）。
pub async fn run<F, Fut>(tool: &str, input: &Value, turn: &Turn, mut call: F) -> Outcome
where
    F: FnMut(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Json, HostError>>,
{
    if tool == "OffPeakList" {
        let listed = call("offPeak/list", json!({})).await.map_err(|e| e.message)?;
        let tasks = listed.get("tasks").and_then(Json::as_array).unwrap_or_default();
        let mut output = Json::object();
        output.set("tasks", Json::Array(tasks.iter().map(summary).collect()));
        return finish(output);
    }
    if turn.off_peak_turn {
        return Err(off_peak_turn_denial("OffPeakCreate", None));
    }
    let fields = parse_create(input)?;
    if turn.active_off_peak_task_id.as_deref().is_some_and(|id| !id.trim().is_empty()) {
        return Err(FROM_OFF_PEAK_RUN.into());
    }
    // 绑定守卫：本会话已有未终态任务即拒绝；查询失败 fail-closed（TS 不把未知当作未绑定）。
    let listed = call("offPeak/list", json!({})).await.map_err(|_| BOUND_CHECK_FAILED.to_owned())?;
    let bound = listed.get("tasks").and_then(Json::as_array).unwrap_or_default().iter().any(|task| {
        task.get("sessionId").and_then(Json::as_str) == Some(turn.session_id.as_str())
            && !task.get("status").and_then(Json::as_str).is_some_and(|s| TERMINAL.contains(&s))
    });
    if bound {
        return Err(IN_BOUND_SESSION.into());
    }
    let mut params = serde_json::Map::new();
    for (key, value) in fields {
        params.insert(key.into(), value.into());
    }
    params.insert("boundSessionId".into(), turn.session_id.clone().into());
    let result = call("offPeak/create", Value::Object(params)).await.map_err(|e| e.message)?;
    if result.get("ok") != Some(&Json::Bool(true)) {
        let text = |key| result.get(key).and_then(Json::as_str).unwrap_or_default();
        return Err(failure_detail(text("errorCategory"), text("errorCode")).into());
    }
    let task = summary(result.get("task").unwrap_or(&Json::Null));
    let id = task.get("offPeakTaskId").and_then(Json::as_str).unwrap_or_default().to_owned();
    let message = match task.get("queuePosition") {
        Some(Json::Number(position)) => format!("Created idle-time task {id} (#{position} in queue)."),
        _ => format!("Created idle-time task {id}."),
    };
    let mut output = Json::object();
    output.set("task", task);
    output.set("message", Json::str(message));
    finish(output)
}

#[cfg(test)]
#[path = "off_peak_tests.rs"]
mod tests;
