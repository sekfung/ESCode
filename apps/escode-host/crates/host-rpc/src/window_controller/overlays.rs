//! sessions-index 摘要 → controller overlay / liveStatus 的映射
//! （TS `windowHostSessionsIndexObserver.ts` + `windowHostControllerService.ts:78-131` 的 `sessionOverlay`）。

use serde_json::{Value, json};

/// TS `sessionOverlay(summary)`：会话摘要 → 投影 overlay（`undefined` 字段整键省略）。
pub(crate) fn session_overlay(summary: &Value) -> Value {
    let mut overlay = json!({
        "taskId": summary["sessionId"],
        "liveStatus": live_status(summary),
    });
    if let Some(title) = summary["title"].as_str() {
        overlay["title"] = Value::from(title);
    }
    if let Some(title_source) = summary["titleSource"].as_str() {
        overlay["titleSource"] = Value::from(title_source);
    }
    if let Some(last_activity_at) = summary["lastActivityAt"].as_i64() {
        overlay["updatedAt"] = Value::from(last_activity_at);
    }
    if let Some(pending) = summary["pendingInteraction"].as_object() {
        let mut interaction = json!({
            "interactionId": pending.get("interactionId").cloned().unwrap_or(Value::Null),
            "kind": pending.get("kind").cloned().unwrap_or(Value::Null),
        });
        if let Some(tool_name) = pending.get("toolName") {
            interaction["toolName"] = tool_name.clone();
        }
        if let Some(auto_resolution) = pending.get("autoResolution") {
            interaction["autoResolution"] = auto_resolution.clone();
        }
        overlay["pendingInteraction"] = interaction;
    }
    let mut activity = json!({
        "phase": summary["phase"],
        "lastActivityAt": summary["lastActivityAt"],
        "hasBackgroundWork": summary["hasBackgroundWork"] == true,
    });
    if let Some(summary_counts) = summary["pendingInteractionSummary"].as_object() {
        activity["pendingInteractions"] = Value::Object(summary_counts.clone());
    }
    if let Some(workflow_activity) = summary.get("workflowActivity") {
        activity["workflowActivity"] = workflow_activity.clone();
    }
    overlay["activity"] = activity;
    overlay
}

/// TS `sessionOverlay` 的 `liveStatus` 判定（等待交互优先于 phase）。
fn live_status(summary: &Value) -> &'static str {
    let waiting_interaction = summary["pendingInteraction"].is_object()
        || summary["pendingInteractionSummary"]
            .as_object()
            .is_some_and(|counts| {
                counts["permissionCount"].as_i64().unwrap_or(0)
                    + counts["userInputCount"].as_i64().unwrap_or(0)
                    > 0
            });
    if waiting_interaction {
        return "waiting";
    }
    match summary["phase"].as_str().unwrap_or_default() {
        "prewarming" | "running" => "running",
        "error" => "error",
        "completedSuccess" | "completedInterrupted" => "completed",
        _ => "idle",
    }
}

/// TS `defaultLiveStatus(meta)`：overlay 缺失表示当前没有可证明的 runtime，
/// SQLite 里的 running/interaction 不能重新解释成当前事实。
pub(crate) fn default_live_status(meta: &Value) -> &'static str {
    match meta["status"].as_str().unwrap_or_default() {
        "completed" => "completed",
        "error" => "error",
        _ => "idle",
    }
}

/// TS `statusFromActivity`：`draft` 相位无对应状态（返回 None → meta 的 `status` 键整键省略）。
pub(crate) fn status_from_activity(activity: &Value) -> Option<&'static str> {
    match activity["phase"].as_str().unwrap_or_default() {
        "prewarming" | "running" => Some("running"),
        "completedSuccess" | "completedInterrupted" => Some("completed"),
        "error" => Some("error"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_interactions_win_over_phase() {
        let summary = json!({
            "sessionId": "t1",
            "phase": "running",
            "lastActivityAt": 7,
            "hasBackgroundWork": false,
            "pendingInteraction": {"interactionId": "i1", "kind": "permission", "toolName": "Bash"},
        });
        let overlay = session_overlay(&summary);
        assert_eq!(overlay["liveStatus"], "waiting");
        assert_eq!(overlay["taskId"], "t1");
        assert_eq!(overlay["updatedAt"], 7);
        assert_eq!(overlay["pendingInteraction"]["toolName"], "Bash");
        assert_eq!(overlay["activity"]["phase"], "running");
        assert!(overlay.get("title").is_none());
    }

    #[test]
    fn pending_interaction_summary_counts_as_waiting_only_when_non_zero() {
        let mut summary = json!({
            "sessionId": "t1",
            "phase": "completedSuccess",
            "lastActivityAt": 1,
            "hasBackgroundWork": false,
            "pendingInteractionSummary": {"permissionCount": 0, "userInputCount": 0},
        });
        assert_eq!(session_overlay(&summary)["liveStatus"], "completed");
        summary["pendingInteractionSummary"]["userInputCount"] = json!(1);
        assert_eq!(session_overlay(&summary)["liveStatus"], "waiting");
    }

    #[test]
    fn draft_phase_drops_the_status_key() {
        assert_eq!(status_from_activity(&json!({"phase": "draft"})), None);
        assert_eq!(
            status_from_activity(&json!({"phase": "error"})),
            Some("error")
        );
        assert_eq!(default_live_status(&json!({"status": "running"})), "idle");
        assert_eq!(
            default_live_status(&json!({"status": "completed"})),
            "completed"
        );
    }
}
