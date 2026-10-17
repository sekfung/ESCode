//! `escode-task.goalSession`（S9）：legacy action 面 → Rust runtime 的 V4 goal 命令。
//!
//! 映射（docs/specs/rust-u2-tasks-index.md §14.2）：
//! `set`/`replace` → `sendGoalCommand`；`pause`/`resume` → CAS `pauseGoal`/`resumeGoal`；
//! `show` 只读快照；`clear` 无运行时命令，回稳定 unsupported code。

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
    match action {
        "set" | "replace" => {
            let objective = params["objective"].as_str().unwrap_or_default();
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
        }
        "pause" => {
            cas(context, target, "pauseGoal", json!({})).await?;
        }
        "resume" => {
            cas(context, target, "resumeGoal", json!({})).await?;
            if let Some(input_id) = input_id {
                context.task_events.run_started(target, input_id);
            }
        }
        "show" => {}
        "clear" => {
            return Err(RpcError::new(
                "Error",
                "Goal clear is not supported by the Rust runtime yet.",
            )
            .with("code", Value::from("ESCODE_GOAL_ACTION_UNSUPPORTED")));
        }
        _ => {
            return Err(RpcError::new(
                "InvalidRequest",
                format!("Unknown goal action: {action}"),
            ));
        }
    }
    let snapshot = resume_and_index(context, target).await?;
    let mut result = json!({"response": "", "snapshot": snapshot});
    if snapshot["session"]["status"] == "running" {
        result["startedTurn"] = Value::Bool(true);
    }
    Ok(result)
}
