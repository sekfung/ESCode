//! `sendPrompt` 与命令队列 drain 共用的发送载荷（S8 从 `domains_task_input.rs` 拆出）。
//!
//! 主路径 V4 `sendText`（`commandId = traceId`）；带附件回退 legacy `session/send`（TS 同）。

use serde_json::{Value, json};

use crate::domains_task_input::{
    prompt_tool_denylist, routing, runtime, send_command, task_target, turn_attribution,
};
use crate::domains_task_state::{StateContext, method};
use crate::forward::{self, Extract, Shaped};
use crate::host_command::{self, HostCommand};
use crate::session::{MethodHandler, RpcError};
use crate::task_targets::TaskTarget;
use crate::wire::Wire;

pub(super) fn send_prompt(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        send_prompt_params(&context, &target, &params).await?;
        Ok(Wire::Undefined)
    })
}

/// TS `sendPromptToAgent`：发送前登记活跃 inputId，失败时清掉（成功保持到 phase 终态）。
pub(super) async fn send_prompt_params(
    context: &StateContext,
    target: &TaskTarget,
    params: &Value,
) -> Result<(), RpcError> {
    let trace_id = params["traceId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("InvalidRequest", "traceId is required."))?
        .to_owned();
    context.commands.set_active(target, &trace_id);
    match send_prompt_payload(context, target, params, &trace_id).await {
        Ok(()) => {
            context.task_events.run_started(target, &trace_id);
            Ok(())
        }
        Err(error) => {
            context.commands.clear_active(target);
            Err(error)
        }
    }
}

/// 队列 drain 与 `sendPrompt` 共用同一发送路径（TS `drainRuntimeCommands` 的载荷子集）。
pub(super) async fn deliver_task_command(
    context: &StateContext,
    target: &TaskTarget,
    command: &Value,
) -> Result<(), RpcError> {
    let mut params = json!({
        "traceId": command["traceId"].clone(),
        "content": command["content"].clone(),
        "messageId": command["commandId"].clone(),
    });
    for key in ["queryId", "automationId", "clientId"] {
        if let Some(value) = command.get(key).filter(|value| !value.is_null()) {
            params[key] = value.clone();
        }
    }
    if let Some(attachments) = command.get("attachments").filter(|value| value.is_array()) {
        params["attachments"] = attachments.clone();
    }
    send_prompt_params(context, target, &params).await
}

async fn send_prompt_payload(
    context: &StateContext,
    target: &TaskTarget,
    params: &Value,
    trace_id: &str,
) -> Result<(), RpcError> {
    let content = params["content"].as_str().unwrap_or_default().to_owned();
    let denylist = prompt_tool_denylist(params);
    let attachments = params["attachments"]
        .as_array()
        .filter(|items| !items.is_empty());
    if let Some(attachments) = attachments {
        // 遗留附件路径：V4 sendText 的附件命令面尚未建模（TS 同）。
        let runtime = runtime(context)?;
        let mut body = json!({
            "sessionId": target.task_id,
            "inputId": trace_id,
            "content": content,
            "attachments": attachments,
        });
        for key in ["queryId", "messageId", "modelSelection", "modelExecution"] {
            if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
                body[key] = value.clone();
            }
        }
        if !denylist.is_empty() {
            body["toolDenylist"] = json!(denylist);
        }
        if let Some(delivery) = params
            .get("botDeliveryTarget")
            .filter(|value| !value.is_null())
        {
            body["botDeliveryTarget"] = delivery.clone();
        }
        for (key, value) in turn_attribution(params)
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            body[key] = value;
        }
        let routing = routing(target);
        let answer = forward::runtime_call_shaped(
            &runtime.instances,
            &runtime.options,
            routing,
            Shaped {
                method: "session/send",
                body,
                with_workspace: false,
                trace: None,
            },
            Extract::Result,
        )
        .await?;
        let _ = answer;
        return Ok(());
    }
    let mut payload = json!({
        "text": content,
        "heldQueueDisposition": "keepQueueAndSend",
    });
    if let Some(model) = params
        .get("modelSelection")
        .filter(|value| !value.is_null())
    {
        payload["modelSelection"] = model.clone();
    }
    if let Some(execution) = params
        .get("modelExecution")
        .filter(|value| !value.is_null())
    {
        payload["modelExecution"] = execution.clone();
    }
    if let Some(delivery) = params
        .get("botDeliveryTarget")
        .filter(|value| !value.is_null())
    {
        payload["botDeliveryTarget"] = delivery.clone();
    }
    if !denylist.is_empty() {
        payload["toolDisallowlist"] = json!(denylist);
    }
    for (key, value) in turn_attribution(params)
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        payload[key] = value;
    }
    let mut command = HostCommand::new("sendText", Some(&target.task_id), payload);
    command.command_id = Some(trace_id.to_owned());
    if let Some(client) = params["clientId"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        command.client_id = Some(client.to_owned());
    }
    let envelope = host_command::envelope(command)?;
    send_command(context, target, envelope, "sendText").await?;
    Ok(())
}
