//! `escode-task` 任务控制与 legacy 桩（S7）：`setMode` / `setModel` / `setConfigOption` /
//! `setAutomationSessionConfig` / `setAssistantMessageFeedback`，以及 TS 适配器本身就是
//! `unsupported` 的方法（`branchTaskFromPrompt` / `deliverSessionMessage` /
//! `sendSessionMessageDeliveryResult` / `switchAgent`；`syncTaskSessionBinding` 在 S8 补齐）。

use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_services::task_projection::{settings_to_config_options, sync_snapshot_to_index};

use crate::deferred_drafts::DeferredDrafts;
use crate::domains_session_write::{SessionWriteContext, resume_snapshot};
use crate::domains_task_create::{
    call_session, parse_model_picker, read_snapshot, to_session_mode,
};
use crate::domains_task_input::{runtime, send_v4, task_target};
use crate::domains_task_state::{StateContext, method};
use crate::domains_task_support::context_ref;
use crate::host_command;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::task_targets::TaskTarget;
use crate::wire::Wire;

const MODEL_CONFIG_ID: &str = "model";
const MODE_CONFIG_ID: &str = "mode";
const THOUGHT_LEVEL_CONFIG_ID: &str = "thought_level";

pub(super) fn control_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("setMode", set_mode(context)),
        ("setModel", set_model(context)),
        ("setConfigOption", set_config_option(context)),
        (
            "setAutomationSessionConfig",
            set_automation_session_config(context),
        ),
        (
            "setAssistantMessageFeedback",
            set_assistant_message_feedback(context),
        ),
        ("branchTaskFromPrompt", unsupported("branchTaskFromPrompt")),
        (
            "deliverSessionMessage",
            unsupported("deliverSessionMessage"),
        ),
        (
            "sendSessionMessageDeliveryResult",
            unsupported("sendSessionMessageDeliveryResult"),
        ),
        ("switchAgent", unsupported("switchAgent")),
        (
            "syncTaskSessionBinding",
            unsupported("syncTaskSessionBinding"),
        ),
    ]
}

/// TS `unsupported`：同名 Error + 稳定 code。
fn unsupported(name: &'static str) -> MethodHandler {
    handler_rpc(move |_arg: Wire| {
        Box::pin(async move {
            Err(RpcError::new(
                "Error",
                format!(
                    "ESCode task service adapter does not support IESCodeTaskService.{name} yet."
                ),
            )
            .with(
                "code",
                Value::from("ESCODE_AGENT_UNSUPPORTED_LEGACY_TASK_METHOD"),
            ))
        })
    })
}

/// 任务控制的 CAS 发送（TS `sendHostCasCommandV4`，默认 4 次）。
pub(super) async fn cas(
    context: &StateContext,
    target: &TaskTarget,
    kind: &str,
    payload: Value,
) -> Result<Value, RpcError> {
    let context = context.clone();
    let target = target.clone();
    let session_id = target.task_id.clone();
    let context_message = format!("session={session_id}");
    let send = move |envelope: Value| {
        let context = context.clone();
        let target = target.clone();
        async move { send_v4(&context, &target, envelope).await }
    };
    host_command::send_cas(send, kind, payload, &session_id, &context_message, 4).await
}

/// `session/resume` → 索引（不发列表事件；控制面的事件由 V4 config delta 驱动）。
pub(super) async fn resume_and_index(
    context: &StateContext,
    target: &TaskTarget,
) -> Result<Value, RpcError> {
    let write = SessionWriteContext {
        runtime: runtime(context)?,
        read: Some(context.read.clone()),
        drafts: Arc::new(DeferredDrafts::new()),
        events: Arc::clone(&context.events),
    };
    let params = json!({
        "sessionId": target.task_id,
        "workspacePath": target.workspace_path,
        "workspaceIdentity": target.workspace_identity,
    });
    let (snapshot, overrides) = resume_snapshot(&write, &params, &params).await?;
    let owned = snapshot.clone();
    context_ref(&context.read)
        .run(move |index| {
            sync_snapshot_to_index(index, &owned, &overrides).map_err(|error| error.to_string())
        })
        .await
        .map_err(RpcError::from)?;
    Ok(snapshot)
}

fn options(snapshot: &Value) -> Wire {
    Wire::Array(
        settings_to_config_options(&snapshot["settings"])
            .into_iter()
            .map(Wire::Object)
            .collect(),
    )
}

/// TS `switchCollaborationModeViaProtocol`：auto 走 legacy `session/setMode`，其余 CAS。
async fn switch_mode(
    context: &StateContext,
    target: &TaskTarget,
    mode: &str,
) -> Result<(), RpcError> {
    if mode == "auto" {
        let runtime = runtime(context)?;
        let params = json!({"sessionId": target.task_id, "mode": mode});
        call_session(&runtime, &runtime_routing(target), "setMode", params, false).await?;
        return Ok(());
    }
    // Rust runtime 的 V4 命令用 `planEnabled` 表达 plan（mode 只接受 build/edit/guarded/yolo）；
    // Node runtime 接受 mode:"plan"，这里做同样的语义适配（TS V4 命令面同义）。
    let payload = if mode == "plan" {
        json!({"mode": "build", "planEnabled": true})
    } else {
        json!({"mode": mode, "planEnabled": false})
    };
    cas(context, target, "switchCollaborationMode", payload).await?;
    Ok(())
}

fn runtime_routing(target: &TaskTarget) -> Value {
    let mut routing = json!({"workspacePath": target.workspace_path});
    if let Some(identity) = &target.workspace_identity {
        routing["workspaceIdentity"] = Value::from(identity.clone());
    }
    routing
}

fn set_mode(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let mode = to_session_mode(params["mode"].as_str()).unwrap_or("build");
        switch_mode(&context, &target, mode).await?;
        resume_and_index(&context, &target).await?;
        Ok(Wire::Undefined)
    })
}

fn set_model(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let runtime = runtime(&context)?;
        let selection = params["modelSelection"].clone();
        call_session(
            &runtime,
            &runtime_routing(&target),
            "setModel",
            json!({"sessionId": target.task_id, "model": selection}),
            false,
        )
        .await?;
        let snapshot = resume_and_index(&context, &target).await?;
        Ok(options(&snapshot))
    })
}

fn set_config_option(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        set_config_option_impl(context, params).await
    })
}

pub(crate) async fn set_config_option_impl(
    context: StateContext,
    params: Value,
) -> crate::session::RpcResult {
    {
        let target = task_target(&context, &params)?;
        let config_id = params["configId"].as_str().unwrap_or_default();
        match config_id {
            MODEL_CONFIG_ID => {
                let value = params["value"].as_str().unwrap_or_default();
                let selection = parse_model_picker(value).ok_or_else(|| {
                    RpcError::new("InvalidRequest", format!("Invalid model value: {value}"))
                })?;
                let runtime = runtime(&context)?;
                call_session(
                    &runtime,
                    &runtime_routing(&target),
                    "setModel",
                    json!({"sessionId": target.task_id, "model": selection}),
                    true,
                )
                .await?;
            }
            THOUGHT_LEVEL_CONFIG_ID => {
                let runtime = runtime(&context)?;
                let current =
                    read_snapshot(&runtime, &runtime_routing(&target), &target.task_id).await?;
                let model = &current["settings"]["model"]["current"];
                let (Some(provider), Some(model_id)) =
                    (model["providerId"].as_str(), model["modelId"].as_str())
                else {
                    return Err(RpcError::new("Error", "请先选择模型，再设置思考档位"));
                };
                cas(
                    &context,
                    &target,
                    "switchModelConfig",
                    json!({
                        "provider": provider,
                        "model": model_id,
                        "thought": params["value"].as_str().unwrap_or_default(),
                    }),
                )
                .await?;
            }
            MODE_CONFIG_ID => {
                let mode = to_session_mode(params["value"].as_str()).unwrap_or("build");
                switch_mode(&context, &target, mode).await?;
            }
            _ => {}
        }
        let snapshot = resume_and_index(&context, &target).await?;
        Ok(options(&snapshot))
    }
}

fn set_automation_session_config(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        set_automation_session_config_impl(context, params).await
    })
}

pub(crate) async fn set_automation_session_config_impl(
    context: StateContext,
    params: Value,
) -> crate::session::RpcResult {
    {
        let target = task_target(&context, &params)?;
        let model = params["modelSelection"].clone();
        let provider = model["providerId"].as_str().unwrap_or_default();
        let model_id = model["modelId"].as_str().unwrap_or_default();
        let thought = params["thoughtLevel"].as_str().unwrap_or_default().trim();
        let payload = json!({"provider": provider, "model": model_id, "thought": thought});
        cas(&context, &target, "switchModelConfig", payload.clone()).await?;
        if !thought.is_empty() {
            // TS：模型事件落地后再以同模型命令显式收敛 thought（同默认值时为 noop）。
            cas(&context, &target, "switchModelConfig", payload).await?;
        }
        let snapshot = resume_and_index(&context, &target).await?;
        Ok(options(&snapshot))
    }
}

fn set_assistant_message_feedback(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let read = context_ref(&context.read);
        let snapshot_handler = crate::domains_task_snapshot::snapshot_handler(
            read,
            Arc::clone(&context.commands),
            false,
        );
        let arg = Wire::Array(vec![Wire::Object(params.clone())]);
        let answered = snapshot_handler(arg).await?;
        let Wire::Object(mut snapshot) = answered else {
            return Err(RpcError::new(
                "Fatal",
                "getTaskSnapshot returned a non-object",
            ));
        };
        let turn_index = params["turnIndex"].as_i64().unwrap_or(-1);
        if let Some(messages) = snapshot["messages"].as_array_mut() {
            let mut assistant_index = 0_i64;
            for message in messages.iter_mut() {
                if message["role"].as_str() != Some("assistant") {
                    continue;
                }
                if assistant_index == turn_index {
                    if params["feedback"].is_null() {
                        if let Some(record) = message.as_object_mut() {
                            record.remove("feedback");
                        }
                    } else {
                        message["feedback"] = params["feedback"].clone();
                    }
                    break;
                }
                assistant_index += 1;
            }
        }
        Ok(Wire::Object(snapshot))
    })
}
