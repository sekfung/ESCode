//! automation run 的真实派发：绑定会话 resume 或新建任务 → 应用运行参数 → sendPrompt。
//! manual（`runAutomationNow`）与常驻 scheduler 共用同一条路径。

use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::channel::EventStream;
use crate::domains_task_control::{set_automation_session_config_impl, set_config_option_impl};
use crate::domains_task_create::create_task_impl;
use crate::domains_task_lifecycle::resume_task_impl;
use crate::domains_task_send::send_prompt_params;
use crate::domains_task_state::StateContext;
use crate::session::RpcError;
use crate::task_targets::TaskTarget;
use crate::wire::Wire;

pub(crate) struct Dispatched {
    pub task_id: String,
    /// 本轮 turn 的终态事件流；调用方据此回写 automation run 的 outcome。
    pub terminal: EventStream,
}

pub(crate) async fn dispatch_run(
    state: &StateContext,
    automation: &Value,
    run: &Value,
) -> Result<Dispatched, RpcError> {
    let run_id = run["runId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("Error", "automation run has no runId"))?;
    let automation_id = automation["automationId"].as_str().unwrap_or_default();
    let workspace_path = automation["workspacePath"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let workspace_identity = automation["workspaceIdentity"].as_str().map(str::to_owned);
    let selection = run
        .get("modelSelection")
        .filter(|value| value.is_object())
        .or_else(|| {
            automation
                .get("modelSelection")
                .filter(|value| value.is_object())
        })
        .cloned();
    let mode = automation["mode"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    let target_task_id = automation["targetTaskId"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let task_id = match target_task_id {
        Some(task_id) => {
            // 绑定会话在重启/切换 workspace 后通常不在活动态：先 resume，再应用保存的模型/模式。
            let mut resume = json!({
                "workspacePath": workspace_path,
                "taskId": task_id,
                "automationId": automation_id,
            });
            if let Some(identity) = &workspace_identity {
                resume["workspaceIdentity"] = Value::from(identity.clone());
            }
            if let Some(selection) = &selection {
                resume["modelSelection"] = selection.clone();
                if let Some(level) = selection["options"]["reasoningLevel"].as_str() {
                    resume["thoughtLevel"] = Value::from(level);
                }
            }
            resume_task_impl(state.clone(), resume).await?;
            apply_run_config(state, &task_id, selection.as_ref(), mode.as_deref()).await;
            task_id
        }
        None => {
            let mut create = json!({
                "workspacePath": workspace_path,
                "automationId": automation_id,
            });
            if let Some(identity) = &workspace_identity {
                create["workspaceIdentity"] = Value::from(identity.clone());
            }
            if let Some(mode) = &mode {
                create["mode"] = Value::from(mode.clone());
            }
            if let Some(selection) = &selection {
                create["modelSelection"] = selection.clone();
            }
            let task = create_task_impl(state, create).await?;
            let task = task
                .into_object()
                .ok_or_else(|| RpcError::new("Fatal", "createTask returned a non-object result"))?;
            task["taskId"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| RpcError::new("Fatal", "createTask result has no taskId"))?
                .to_owned()
        }
    };

    let target = TaskTarget {
        task_id: task_id.clone(),
        workspace_path,
        workspace_identity,
        cron_automation_id: Some(automation_id.to_owned()),
        remote_session_id: None,
    };
    // 订阅必须先于 sendPrompt：快速终态不会落在订阅之前。
    let terminal = state.task_events.listen_terminal(&task_id);
    let send = json!({
        "traceId": run_id,
        "content": automation["prompt"],
        "automationId": automation_id,
        "clientMode": "desktop-continuous",
    });
    send_prompt_params(state, &target, &send).await?;
    Ok(Dispatched { task_id, terminal })
}

/// TS `applyCronRunConfigToExistingTask`：模型选择优先，其次单独应用模式/思考档位。
async fn apply_run_config(
    state: &StateContext,
    task_id: &str,
    selection: Option<&Value>,
    mode: Option<&str>,
) {
    if let Some(selection) = selection {
        let mut params = json!({
            "taskId": task_id,
            "modelSelection": selection,
        });
        if let Some(level) = selection["options"]["reasoningLevel"].as_str() {
            params["thoughtLevel"] = Value::from(level);
        }
        let _ = set_automation_session_config_impl(state.clone(), params).await;
    } else if let Some(mode) = mode {
        let _ = set_config_option_impl(
            state.clone(),
            json!({"taskId": task_id, "configId": "mode", "value": mode}),
        )
        .await;
    }
}

/// 只接受本轮 runId 的终态；旧回合/其它输入的终态不能改写本次 run。
fn outcome_for_run(value: &Value, run_id: &str) -> Option<(String, Option<String>)> {
    let input = value["inputId"]
        .as_str()
        .or_else(|| value["traceId"].as_str());
    if input != Some(run_id) {
        return None;
    }
    Some((
        value["outcome"].as_str().unwrap_or("failed").to_owned(),
        value["error"].as_str().map(str::to_owned),
    ))
}

/// 等待本轮终态并回写 automation run 的 outcome（按 inputId 过滤，避免误吞旧回合）。
pub(crate) fn track_outcome(
    service: std::sync::Arc<escode_host_services::automation::AutomationService>,
    run_id: String,
    mut terminal: EventStream,
) {
    tokio::spawn(async move {
        while let Some(event) = terminal.next().await {
            let Wire::Object(value) = event else {
                continue;
            };
            let Some((outcome, error)) = outcome_for_run(&value, &run_id) else {
                continue;
            };
            let _ = service.mark_run_outcome(run_id, outcome, error).await;
            break;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_outcome_is_scoped_to_the_run_id() {
        let event = json!({
            "taskId": "task-1",
            "inputId": "run-1",
            "outcome": "succeeded",
        });
        assert_eq!(
            outcome_for_run(&event, "run-1"),
            Some(("succeeded".to_owned(), None))
        );
        assert_eq!(outcome_for_run(&event, "run-2"), None);
    }

    #[test]
    fn failed_terminal_outcome_keeps_the_error() {
        let event = json!({
            "taskId": "task-1",
            "traceId": "run-1",
            "outcome": "failed",
            "error": "boom",
        });
        assert_eq!(
            outcome_for_run(&event, "run-1"),
            Some(("failed".to_owned(), Some("boom".to_owned())))
        );
    }
}
