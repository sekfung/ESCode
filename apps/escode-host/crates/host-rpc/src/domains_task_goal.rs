//! `escode-task.goalSession`（S9）：legacy action 面 → Rust runtime 的 V4 goal 命令。
//!
//! 映射（docs/specs/rust-u2-tasks-index.md §14.2）：
//! `set`/`replace` → `sendGoalCommand`；`pause`/`resume`/`clear` → CAS
//! `pauseGoal`/`resumeGoal`/`clearGoal`；`show` 只读快照，`response` 按 TS `formatGoalSummary`
//! 从 legacy 快照 `session.target` 生成。

use serde_json::{Value, json};

use crate::domains_task_control::{cas, resume_and_index};
use crate::domains_task_input::{send_command, task_target};
use crate::domains_task_state::{StateContext, method};
use crate::host_command::{self, HostCommand};
use crate::session::{MethodHandler, RpcError};
use crate::wire::Wire;

pub(super) fn goal_method(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let action = params["action"].as_str().unwrap_or_default();
        let input_id = params["inputId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let may_start = matches!(action, "set" | "replace" | "resume");
        if may_start && let Some(input_id) = input_id.as_ref() {
            context.commands.set_active(&target, input_id);
        }
        let result = goal_payload(&context, &target, &params, action, input_id.as_deref()).await;
        match &result {
            Ok(snapshot) => {
                let running = snapshot["session"]["status"] == "running";
                if !may_start || !running {
                    context.commands.clear_active(&target);
                }
            }
            Err(_) => context.commands.clear_active(&target),
        }
        result.map(Wire::Object)
    })
}

async fn goal_payload(
    context: &StateContext,
    target: &crate::task_targets::TaskTarget,
    params: &Value,
    action: &str,
    input_id: Option<&str>,
) -> Result<Value, RpcError> {
    // 各 action 的 response 文案对齐 TS goalSession；目标文本在命令之后从快照 `session.target` 读取。
    let response: fn(&Value) -> String = match action {
        "set" | "replace" => {
            let objective = params["objective"].as_str().unwrap_or_default().trim();
            if objective.is_empty() {
                let snapshot = resume_and_index(context, target).await?;
                let usage = if action == "replace" {
                    "Usage: /goal replace <objective>"
                } else {
                    "Usage: /goal <objective>"
                };
                return Ok(json!({"response": usage, "snapshot": snapshot, "startedTurn": false}));
            }
            let mut command = HostCommand::new(
                "sendGoalCommand",
                Some(&target.task_id),
                json!({"text": objective}),
            );
            command.command_id = input_id.map(str::to_owned);
            let envelope = host_command::envelope(command)?;
            send_command(context, target, envelope, "sendGoalCommand").await?;
            if let Some(input_id) = input_id {
                context.task_events.run_started(target, input_id);
            }
            |goal| goal_changed("Goal active", goal)
        }
        "pause" => {
            let ack = cas(context, target, "pauseGoal", json!({})).await?;
            // TS：暂停成功不回文案（避免空 assistant 气泡），无可暂停目标才提示。
            if ack["status"] == "accepted" {
                |_| String::new()
            } else {
                |_| "No goal to pause.".into()
            }
        }
        "resume" => {
            let ack = cas(context, target, "resumeGoal", json!({})).await?;
            if ack["status"] != "accepted" {
                |_| "No goal to resume.".into()
            } else {
                if let Some(input_id) = input_id {
                    context.task_events.run_started(target, input_id);
                }
                |goal| goal_changed("Goal resumed", goal)
            }
        }
        "show" => |goal| goal_summary(goal),
        "clear" => {
            let ack = cas(context, target, "clearGoal", json!({})).await?;
            if ack["status"] == "accepted" {
                |_| "Goal cleared.".into()
            } else {
                |_| "No goal to clear.".into()
            }
        }
        _ => {
            return Err(RpcError::new(
                "InvalidRequest",
                format!("Unknown goal action: {action}"),
            ));
        }
    };
    let snapshot = resume_and_index(context, target).await?;
    let started = matches!(action, "set" | "replace" | "resume")
        && snapshot["session"]["status"] == "running";
    Ok(json!({
        "response": response(&snapshot["session"]["target"]),
        "snapshot": snapshot,
        "startedTurn": started,
    }))
}

/// TS `formatGoalSummary`（`target` 即 legacy 快照的 `session.target`）。
fn goal_summary(target: &Value) -> String {
    if target["objective"].as_str().is_none() {
        return "No goal is set. Use /goal <objective> to set one.".into();
    }
    goal_changed(
        &format!("Goal {}", target["status"].as_str().unwrap_or_default()),
        target,
    )
}

/// TS `formatGoalChanged`。
fn goal_changed(title: &str, target: &Value) -> String {
    let objective = target["objective"].as_str().unwrap_or_default();
    let mut lines = vec![title.to_owned(), format!("Objective: {objective}")];
    let used = target.get("tokensUsed").filter(|v| !v.is_null());
    let budget = target.get("tokenBudget");
    if used.is_some() || budget.is_some() {
        let budget = budget
            .and_then(Value::as_u64)
            .map_or_else(|| "none".to_owned(), |b| b.to_string());
        let used = used.and_then(Value::as_u64).unwrap_or(0);
        lines.push(format!("Usage: {used} tokens / {budget}"));
    }
    if let Some(seconds) = target["timeUsedSeconds"].as_u64() {
        lines.push(format!("Time: {seconds} seconds"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::goal_summary;
    use serde_json::{Value, json};

    #[test]
    fn summary_matches_ts_format_goal_summary() {
        assert_eq!(
            goal_summary(&Value::Null),
            "No goal is set. Use /goal <objective> to set one."
        );
        assert_eq!(
            goal_summary(&json!({"objective":"ship","status":"paused","timeUsedSeconds":12})),
            "Goal paused\nObjective: ship\nTime: 12 seconds"
        );
        assert_eq!(
            goal_summary(&json!({"objective":"ship","status":"active","tokensUsed":7,
                "tokenBudget":null,"timeUsedSeconds":0})),
            "Goal active\nObjective: ship\nUsage: 7 tokens / none\nTime: 0 seconds"
        );
    }
}
