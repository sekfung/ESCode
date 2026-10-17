//! `escode-task` Bot group 7 方法（S12）：V4 快照观察、队列取消、执行停止与提交输入。
//!
//! 运行时事实源是 V4 conversation snapshot（control/queue/pendingInteractions）；
//! Host 只做授权、admission 与附件事务，不复制第二份输入队列。

use std::sync::Arc;
use std::time::Duration;

use escode_host_services::tasks_index::workspace_key;
use serde_json::{Value, json};

use crate::agent_v4::base::Base;
use crate::agent_v4::observe::conversation_snapshot;
use crate::domains_task_state::{StateContext, method};
use crate::forward::{self, Extract, RuntimePolicy};
use crate::host_command::{self, HostCommand};
use crate::session::{MethodHandler, RpcError};
use crate::task_targets::TaskTarget;
use crate::wire::Wire;

pub(super) fn bot_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("readBotTopicSummaries", read_summaries(context)),
        ("invalidateBotTopicInputs", invalidate(context)),
        ("readBotTopicExecution", read_execution(context)),
        ("stopBotTopicExecution", stop_execution(context)),
        ("getBotGroupTaskBlockReason", block_reason(context)),
        ("cancelBotGroupInput", cancel_input(context)),
        (
            "submitBotGroupInput",
            super::domains_task_bot_submit::submit_input(context),
        ),
    ]
}

/// `onDynamicConversationFrames`：Bots 回复流的 V4 帧源（docs/specs/bots-v4-stream.md）。
pub(super) fn conversation_frames(context: &StateContext) -> crate::session::EventHandler {
    let context = context.clone();
    Arc::new(move |arg: Wire| {
        let params = arg
            .into_args()
            .into_iter()
            .next()
            .and_then(Wire::into_object)
            .unwrap_or(Value::Null);
        let target = resolve_target(&context, &params, "Group task workspace mismatch")
            .map_err(|error| error.message)?;
        let base = observer(&context).map_err(|error| error.message)?;
        Ok(crate::agent_v4::observe::conversation_frames(
            base,
            v4_target(&target, &params),
        ))
    })
}

pub(super) fn observer(context: &StateContext) -> Result<Arc<Base>, RpcError> {
    context
        .observer
        .clone()
        .ok_or_else(|| RpcError::new("InvalidRequest", "Host has no runtime access."))
}

fn target_key(target: &TaskTarget) -> String {
    format!(
        "{}\0{}",
        workspace_key(&target.workspace_path, target.workspace_identity.as_deref()),
        target.task_id
    )
}

/// TS `getTaskTarget` + workspace mismatch 守卫。
pub(super) fn resolve_target(
    context: &StateContext,
    params: &Value,
    mismatch: &str,
) -> Result<TaskTarget, RpcError> {
    let task_id = params["taskId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("InvalidRequest", "taskId is required."))?;
    let registered = context.read.targets.get(task_id).ok();
    let provided_path = params["workspacePath"]
        .as_str()
        .filter(|value| !value.is_empty());
    if let Some(registered) = registered.as_ref() {
        if let Some(path) = provided_path {
            let requested = target_from_params(path, params, task_id);
            if workspace_key(
                &registered.workspace_path,
                registered.workspace_identity.as_deref(),
            ) != workspace_key(
                &requested.workspace_path,
                requested.workspace_identity.as_deref(),
            ) {
                return Err(RpcError::new("Error", mismatch));
            }
        }
        return Ok(registered.clone());
    }
    match provided_path {
        Some(path) => Ok(target_from_params(path, params, task_id)),
        None => context
            .read
            .targets
            .get(task_id)
            .map_err(|(code, message)| RpcError::new(code, message)),
    }
}

fn target_from_params(path: &str, params: &Value, task_id: &str) -> TaskTarget {
    TaskTarget {
        task_id: task_id.to_owned(),
        workspace_path: path.to_owned(),
        workspace_identity: params["workspaceIdentity"].as_str().map(str::to_owned),
        cron_automation_id: None,
        remote_session_id: params["remoteSessionId"].as_str().map(str::to_owned),
    }
}

pub(super) fn v4_target(target: &TaskTarget, params: &Value) -> Value {
    let mut value = json!({
        "workspacePath": target.workspace_path,
        "sessionId": target.task_id,
    });
    if let Some(identity) = target.workspace_identity.as_ref() {
        value["workspaceIdentity"] = Value::from(identity.clone());
    }
    if let Some(remote) = params["remoteSessionId"]
        .as_str()
        .or(target.remote_session_id.as_deref())
    {
        value["remoteSessionId"] = Value::from(remote);
    }
    value
}

async fn snapshot(context: &StateContext, target: &Value) -> Result<Value, RpcError> {
    conversation_snapshot(&observer(context)?, target).await
}

fn text_from_parts(parts: &Value) -> String {
    parts
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|part| part["type"] == "text" && part["ignored"] != true)
                .map(|part| part["text"].as_str().unwrap_or_default())
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn read_summaries(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Topic task workspace mismatch")?;
        let runtime = crate::domains_task_input::runtime(&context)?;
        let result = forward::runtime_call_shaped(
            &runtime.instances,
            &runtime.options,
            v4_target(&target, &params),
            forward::Shaped {
                method: "session/messages",
                body: json!({"sessionId": target.task_id, "limit": 200}),
                with_workspace: true,
                trace: None,
            },
            Extract::Result,
        )
        .await?;
        let Wire::Object(result) = result else {
            return Err(RpcError::new(
                "Fatal",
                "session/messages returned a non-object result",
            ));
        };
        let mut summaries = Vec::new();
        for message in result["messages"].as_array().into_iter().flatten() {
            let info = &message["info"];
            if info["role"] != "user" || info["source"] != "bot_topic_context" {
                continue;
            }
            let checkpoint = info["metadata"]["checkpoint"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let text = info["metadata"]["displayText"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| text_from_parts(&message["parts"]));
            summaries.push(json!({"checkpoint": checkpoint, "text": text}));
        }
        let summaries = summaries.split_off(summaries.len().saturating_sub(20));
        Ok(Wire::Array(
            summaries.into_iter().map(Wire::Object).collect(),
        ))
    })
}

pub(super) fn admission_key(target: &TaskTarget, params: &Value) -> String {
    let remote = params["remoteSessionId"]
        .as_str()
        .or(target.remote_session_id.as_deref());
    json!([target_key(target), remote]).to_string()
}

fn invalidate(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Group task workspace mismatch")?;
        context
            .bot_group
            .invalidate(&admission_key(&target, &params))
            .await;
        Ok(Wire::Undefined)
    })
}

fn read_execution(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Group task workspace mismatch")?;
        let snapshot = snapshot(&context, &v4_target(&target, &params)).await?;
        if snapshot["control"]["stopState"] == "idle" {
            return Ok(Wire::Undefined);
        }
        let execution = snapshot["control"]["activeWorks"]
            .as_array()
            .and_then(|works| {
                works
                    .iter()
                    .find(|work| work["foregroundExecutionId"].as_str().is_some())
            })
            .ok_or_else(|| RpcError::new("Error", "Topic execution identity is unavailable"))?;
        let mut result = json!({
            "executionId": execution["foregroundExecutionId"],
        });
        if let Some(source) = execution["sourceCommandId"].as_str() {
            result["sourceCommandId"] = Value::from(source);
        }
        Ok(Wire::Object(result))
    })
}

fn stop_execution(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Group task workspace mismatch")?;
        let execution_id = params["executionId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RpcError::new("InvalidRequest", "executionId is required."))?;
        let observer = observer(&context)?;
        let routing = v4_target(&target, &params);
        let envelope = host_command::envelope(HostCommand::new(
            "stop",
            Some(&target.task_id),
            json!({"expectedForegroundExecutionId": execution_id}),
        ))?;
        let ack = observer
            .call(
                &routing,
                RuntimePolicy::StartIfNeeded,
                "v4/command",
                envelope,
            )
            .await?
            .result;
        host_command::assert_ack_ok("stop", &ack, &format!("session={}", target.task_id))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let snapshot = conversation_snapshot(&observer, &routing).await?;
            let still_running =
                snapshot["control"]["activeWorks"]
                    .as_array()
                    .is_some_and(|works| {
                        works.iter().any(|work| {
                            work["foregroundExecutionId"].as_str() == Some(execution_id)
                        })
                    });
            if !still_running && snapshot["control"]["stopState"] != "stopping" {
                return Ok(Wire::Undefined);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RpcError::new("Error", "Topic stop timed out"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
}

fn block_reason(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Group task workspace mismatch")?;
        let snapshot = snapshot(&context, &v4_target(&target, &params)).await?;
        let reason = if snapshot["pendingInteractions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            Some("interaction")
        } else if matches!(
            snapshot["control"]["phase"].as_str(),
            Some("running" | "prewarming")
        ) {
            Some("running")
        } else if snapshot["queue"]["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            Some("queued")
        } else {
            None
        };
        Ok(match reason {
            Some(reason) => Wire::Str(reason.to_owned()),
            None => Wire::Object(Value::Null),
        })
    })
}

fn cancel_input(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Group task workspace mismatch")?;
        let observer = observer(&context)?;
        let routing = v4_target(&target, &params);
        let snapshot = conversation_snapshot(&observer, &routing).await?;
        let source_command_id = params["sourceCommandId"].as_str().unwrap_or_default();
        let item = snapshot["queue"]["items"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["sourceCommandId"].as_str() == Some(source_command_id))
            })
            .filter(|item| item["dispatch"]["state"] == "queued")
            .ok_or_else(|| RpcError::new("Error", "Input is no longer queued"))?;
        let source = &item["botGroupSource"];
        let actor = params["actorId"].as_str().unwrap_or_default();
        let owner = params["ownerId"].as_str().unwrap_or_default();
        if source["botId"].as_str() != params["botId"].as_str()
            || source["chatId"].as_str() != params["chatId"].as_str()
            || (actor != owner && source["senderId"].as_str() != Some(actor))
        {
            return Err(RpcError::new(
                "Error",
                "Queue cancellation is not authorized",
            ));
        }
        let mut command = HostCommand::new(
            "deleteQueueItem",
            Some(&target.task_id),
            json!({"queueItemId": item["queueItemId"]}),
        );
        command.command_id = params["commandId"].as_str().map(str::to_owned);
        command.base_revision = snapshot["revision"].as_i64();
        let ack = observer
            .call(
                &routing,
                RuntimePolicy::StartIfNeeded,
                "v4/command",
                host_command::envelope(command)?,
            )
            .await?
            .result;
        Ok(Wire::Object(ack))
    })
}
