//! TS `packages/shared/src/escode-session-task-status.ts` 的 Rust 投影：由会话状态快照派生任务状态。
//!
//! 消费者：任务索引投影与 `session/read` 的桌面连续链路；`hasCompletedVisibleAssistantTurn` 复用
//! `visible.rs::user_visible_messages`（与 TS 同源），否则两侧对「可见 assistant」的判定会漂移。

use super::policy::{js_trim, js_truthy, opt_str};
use super::visible::user_visible_messages;
use serde_json::Value;

/// TS `deriveESCodeTaskStatusFromSessionSnapshot`：`"running"` / `"error"` / `"completed"`，
/// `None` 对应 TS 的 `undefined`（`idle` 等映射不出来的状态）。
pub fn task_status_from_snapshot(snapshot: &Value) -> Option<&'static str> {
    let projection = snapshot.get("projection").and_then(Value::as_object);
    // `if (snapshot.projection.lastError)`：对象（含 `{}`）为真，null/undefined/""/0 为假。
    if projection
        .and_then(|p| p.get("lastError"))
        .is_some_and(js_truthy)
    {
        return Some("error");
    }

    let status = status_from_session(session_status(snapshot));
    if status == Some("error") || status == Some("completed") {
        return status;
    }
    // desktop continuous 的 session/read 依赖 runtime projection；旧投影可能只 replay 到
    // model_streaming finish、漏掉 turn_complete，导致 currentTurnId 残留。此时持久化 assistant 已有
    // completed 时间，比 stale currentTurnId 更权威。
    let blocking = has_blocking_active_runtime(snapshot);
    if has_completed_visible_assistant_turn(snapshot) && !blocking {
        return Some("completed");
    }
    if blocking {
        return status.or(Some("running"));
    }
    status
}

/// TS `statusFromESCodeSession`。
fn status_from_session(status: Option<&str>) -> Option<&'static str> {
    match status {
        Some("running" | "waiting" | "paused") => Some("running"),
        Some("error") => Some("error"),
        Some("completed") => Some("completed"),
        // 含 "idle" 与 schema 外的值：TS 显式 `return undefined`。
        _ => None,
    }
}

fn session_status(snapshot: &Value) -> Option<&str> {
    snapshot.get("session")?.get("status")?.as_str()
}

/// TS `hasActiveSnapshotRuntime`（`hasBlockingActiveSnapshotRuntime` 在 TS 里与它同体）：
/// `projection.currentTurnId` 是最后一次投影的 turn 边界、完成后会保留，只有 runtime active 字段、
/// 权限或工具调用才能证明当前仍有真实阻塞运行态。
fn has_blocking_active_runtime(snapshot: &Value) -> bool {
    let runtime = snapshot.get("runtime").and_then(Value::as_object);
    let turn_active = ["activeTurnId", "activeTurnKind"]
        .iter()
        .any(|key| runtime.and_then(|r| r.get(*key)).is_some_and(js_truthy));
    if turn_active {
        return true;
    }
    let projection = snapshot.get("projection").and_then(Value::as_object);
    let pending_permissions = projection
        .and_then(|p| p.get("pendingPermissions"))
        .and_then(Value::as_array)
        .is_some_and(|list| !list.is_empty());
    if pending_permissions {
        return true;
    }
    projection
        .and_then(|p| p.get("activeToolCalls"))
        .and_then(Value::as_array)
        .is_some_and(|calls| {
            calls.iter().any(|call| {
                matches!(
                    call.get("status").and_then(Value::as_str),
                    Some("pending" | "running")
                )
            })
        })
}

/// TS `hasCompletedVisibleAssistantTurn`：可见投影的最后一条是 assistant、不是 tool-calls 续跑，
/// 且带持久化的完成时间。
fn has_completed_visible_assistant_turn(snapshot: &Value) -> bool {
    let messages: &[Value] = match snapshot.get("messages").and_then(Value::as_array) {
        Some(list) => list,
        None => &[],
    };
    let visible = user_visible_messages(messages);
    let Some(latest) = visible.last() else {
        return false;
    };
    let info = latest.get("info").and_then(Value::as_object);
    if opt_str(info, "role") != Some("assistant") {
        return false;
    }
    if is_tool_call_continuation_finish(opt_str(info, "finish")) {
        return false;
    }
    // `typeof time.completed === "number"`：没有持久化完成时间就是还没完成。
    latest
        .get("info")
        .and_then(|i| i.get("time"))
        .and_then(|t| t.get("completed"))
        .is_some_and(Value::is_number)
}

/// TS `isToolCallContinuationFinish`：`finish?.trim().toLowerCase().replace(/_/g, "-") === "tool-calls"`。
fn is_tool_call_continuation_finish(finish: Option<&str>) -> bool {
    finish.is_some_and(|raw| js_trim(raw).to_lowercase().replace('_', "-") == "tool-calls")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rustfmt::skip]
    fn json(raw: &str) -> Value { serde_json::from_str(raw).expect("json") }
    /// 快照的最小子集：`session.status` / `projection` / `runtime` / `messages`。
    #[rustfmt::skip]
    fn snapshot(status: &str, projection: &str, runtime: &str, messages: &str) -> Value {
        json(&format!("{{\"session\":{{\"status\":\"{status}\"}},\"projection\":{projection},\"runtime\":{runtime},\"messages\":{messages}}}"))
    }
    /// 已完成的可见 assistant（`time.completed` 存在），可追加 finish。
    #[rustfmt::skip]
    fn finished_assistant(finish: &str) -> String {
        format!(r#"[{{"info":{{"role":"assistant","messageId":"a1","time":{{"created":1,"completed":2}}{finish}}},"parts":[]}}]"#)
    }

    #[test]
    #[rustfmt::skip]
    fn last_error_wins() {
        // `projection.lastError` 为真值 → error，优先于 session/runtime。
        let failed = snapshot("running", r#"{"lastError":{"type":"api","message":"boom"}}"#, r#"{"activeTurnId":"t1"}"#, "[]");
        assert_eq!(task_status_from_snapshot(&failed), Some("error"));
        let empty_error = snapshot("idle", r#"{"lastError":{}}"#, "{}", "[]");
        assert_eq!(task_status_from_snapshot(&empty_error), Some("error"));
    }

    #[test]
    #[rustfmt::skip]
    fn session_status_and_active_runtime() {
        // idle 映射不出状态，但 runtime 仍有真实运行态 → running。
        let active = snapshot("idle", "{}", r#"{"activeTurnId":"t1"}"#, "[]");
        assert_eq!(task_status_from_snapshot(&active), Some("running"));
        // waiting/paused 映射成 running；completed 原样；idle 且无运行态 → undefined。
        assert_eq!(task_status_from_snapshot(&snapshot("paused", "{}", "{}", "[]")), Some("running"));
        assert_eq!(task_status_from_snapshot(&snapshot("completed", "{}", "{}", "[]")), Some("completed"));
        assert_eq!(task_status_from_snapshot(&snapshot("idle", "{}", "{}", "[]")), None);
        // 权限/工具调用未收口也算阻塞运行态。
        let permission = snapshot("idle", r#"{"pendingPermissions":[{"requestId":"p","options":[{"id":"allow","label":"Allow"}],"requestedAt":1}]}"#, "{}", "[]");
        assert_eq!(task_status_from_snapshot(&permission), Some("running"));
    }

    #[test]
    #[rustfmt::skip]
    fn completed_visible_turn_beats_stale_projection() {
        // currentTurnId 残留但 assistant 已有 completed 时间、且无阻塞运行态 → completed。
        let stale = snapshot("running", r#"{"currentTurnId":"t1"}"#, "{}", &finished_assistant(""));
        assert_eq!(task_status_from_snapshot(&stale), Some("completed"));
        // tool-calls 续跑 finish 不是完成：回 session status。
        let tool_calls = snapshot("running", "{}", "{}", &finished_assistant(r#","finish":"tool_calls""#));
        assert_eq!(task_status_from_snapshot(&tool_calls), Some("running"));
        // 有阻塞的 activeToolCalls 时同样不算完成。
        let blocking = snapshot("running", r#"{"activeToolCalls":[{"status":"running"}]}"#, "{}", &finished_assistant(""));
        assert_eq!(task_status_from_snapshot(&blocking), Some("running"));
        // 最后一条可见消息是 user（或没有 completed 时间）→ 不算完成。
        let user_last = snapshot("running", "{}", "{}", r#"[{"info":{"role":"user","messageId":"u1","time":{"created":1}},"parts":[]}]"#);
        assert_eq!(task_status_from_snapshot(&user_last), Some("running"));
    }
}
