//! `escode-task` 输入/停止/交互（S6）：`sendPrompt`（发送载荷在 `domains_task_send.rs`）、
//! `stopGeneration`、`compactSession`、`respondPermission`、`respondElicitation`。
//!
//! 主路径统一走 V4 `v4/command`（`sendText` / `stop` / `compact` / `resolveInteraction`）；
//! 带附件的输入回退 legacy `session/send`（与 TS 一致：附件命令面尚未进 V4）。
//! `goalSession` 依赖运行时的 legacy goal actions，当前运行时未提供，留在缺口表。

use serde_json::{Value, json};

use crate::domains_task_state::{StateContext, method};
use crate::domains_task_support::RuntimeAccess;
use crate::forward::{self, Extract, Shaped};
use crate::host_command::{self, HostCommand};
use crate::session::{MethodHandler, RpcError};
use crate::task_targets::TaskTarget;
use crate::wire::Wire;

const AUTOMATION_DENYLIST: [&str; 3] = ["CronCreate", "CronUpdate", "CronDelete"];
const OFF_PEAK_DENYLIST: [&str; 1] = ["OffPeakCreate"];

pub(super) fn input_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("sendPrompt", super::domains_task_send::send_prompt(context)),
        ("stopGeneration", stop_generation(context)),
        ("compactSession", compact_session(context)),
        ("respondPermission", respond_permission(context)),
        ("respondElicitation", respond_elicitation(context)),
    ]
}

pub(super) fn task_target(context: &StateContext, params: &Value) -> Result<TaskTarget, RpcError> {
    let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
    if task_id.is_empty() {
        return Err(RpcError::new("InvalidRequest", "taskId is required."));
    }
    let path = params["workspacePath"]
        .as_str()
        .filter(|path| !path.is_empty());
    if let Some(path) = path {
        return Ok(TaskTarget {
            task_id,
            workspace_path: path.to_owned(),
            workspace_identity: params["workspaceIdentity"].as_str().map(str::to_owned),
            cron_automation_id: None,
            remote_session_id: params["remoteSessionId"].as_str().map(str::to_owned),
        });
    }
    context
        .read
        .targets
        .get(&task_id)
        .map_err(|(code, message)| RpcError::new(code, message))
}

pub(super) fn runtime(context: &StateContext) -> Result<RuntimeAccess, RpcError> {
    context
        .read
        .runtime
        .clone()
        .ok_or_else(|| RpcError::new("InvalidRequest", "Host has no runtime access."))
}

/// TS `turnAttributionOf`：automation 优先于 off-peak。
pub(super) fn turn_attribution(params: &Value) -> Value {
    if let Some(automation) = params["automationId"].as_str() {
        return json!({"automationId": automation});
    }
    if let Some(offpeak) = params["offPeakTaskId"].as_str() {
        let mut value = json!({"offPeakTaskId": offpeak});
        if let Some(kind) = params["offPeakRunType"].as_str() {
            value["offPeakRunType"] = Value::from(kind);
        }
        return value;
    }
    json!({})
}

/// TS `resolvePromptToolDenylist`：基础 denylist + 本轮 automation/off-peak 的纵深隐藏。
pub(super) fn prompt_tool_denylist(params: &Value) -> Vec<String> {
    let mut tools: Vec<String> = params["toolDenylist"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    // TS 两个分支独立执行：automation 轮与闲时轮同时打标时两组工具都要隐藏。
    let mut extra: Vec<&str> = Vec::new();
    if params["automationId"].as_str().is_some() {
        extra.extend(AUTOMATION_DENYLIST);
    }
    if params["offPeakTaskId"].as_str().is_some() {
        extra.extend(OFF_PEAK_DENYLIST);
    }
    for tool in extra {
        if !tools.iter().any(|existing| existing == tool) {
            tools.push(tool.to_owned());
        }
    }
    tools
}

pub(super) fn routing(target: &TaskTarget) -> Value {
    let mut routing = json!({"workspacePath": target.workspace_path});
    if let Some(identity) = &target.workspace_identity {
        routing["workspaceIdentity"] = Value::from(identity.clone());
    }
    routing
}

pub(super) async fn send_v4(
    context: &StateContext,
    target: &TaskTarget,
    envelope: Value,
) -> Result<Value, RpcError> {
    let runtime = runtime(context)?;
    let target = forward::runtime_target(routing(target))
        .map_err(|(name, message)| RpcError::new(name, message))?;
    let instance = forward::runtime_instance(
        &runtime.instances,
        &runtime.options,
        &target,
        forward::RuntimePolicy::StartIfNeeded,
    )
    .await?;
    forward::request(&instance, "v4/command", envelope, None).await
}

pub(super) async fn send_command(
    context: &StateContext,
    target: &TaskTarget,
    envelope: Value,
    kind: &str,
) -> Result<Value, RpcError> {
    let ack = send_v4(context, target, envelope).await?;
    host_command::assert_ack_ok(kind, &ack, &format!("session={}", target.task_id))?;
    Ok(ack)
}

fn stop_generation(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let mut payload = json!({});
        if let Some(execution) = params["expectedForegroundExecutionId"]
            .as_str()
            .filter(|value| !value.is_empty())
        {
            payload["expectedForegroundExecutionId"] = Value::from(execution);
        }
        let envelope =
            host_command::envelope(HostCommand::new("stop", Some(&target.task_id), payload))?;
        send_command(&context, &target, envelope, "stop").await?;
        Ok(Wire::Undefined)
    })
}

fn compact_session(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        // TS compactSession 在 inputId 存在时登记活跃轮次；accepted 分支保持到 phase 终态。
        let active_input_id = params["inputId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if let Some(input_id) = active_input_id.as_ref() {
            context.commands.set_active(&target, input_id);
        }
        match compact_payload(&context, &target, &params).await {
            Ok(result) => {
                if let Some(input_id) = active_input_id.as_ref() {
                    context.task_events.run_started(&target, input_id);
                }
                Ok(result)
            }
            Err(error) => {
                context.commands.clear_active(&target);
                Err(error)
            }
        }
    })
}

async fn compact_payload(
    context: &StateContext,
    target: &TaskTarget,
    params: &Value,
) -> Result<Wire, RpcError> {
    let instructions = params["instructions"].as_str().unwrap_or_default();
    let envelope = host_command::envelope(HostCommand::new(
        "compact",
        Some(&target.task_id),
        json!({"text": instructions}),
    ))?;
    let ack = send_command(context, target, envelope, "compact").await?;
    let snapshot = read_session(context, target).await?;
    let input_id = ack["result"]["inputId"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| params["inputId"].as_str().map(str::to_owned));
    let mut compact = json!({"state": "accepted"});
    if let Some(input_id) = input_id {
        compact["inputId"] = Value::from(input_id);
    }
    Ok(Wire::Object(json!({
        "response": "",
        "snapshot": snapshot,
        "compact": compact,
    })))
}

fn respond_permission(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let request_id = params["requestId"].as_str().unwrap_or_default();
        let envelope = host_command::envelope(HostCommand::new(
            "resolveInteraction",
            Some(&target.task_id),
            json!({
                "interactionId": request_id,
                "answer": {"optionId": params["optionId"].as_str().unwrap_or_default()},
            }),
        ))?;
        send_command(&context, &target, envelope, "resolveInteraction").await?;
        Ok(Wire::Object(Value::Bool(true)))
    })
}

fn respond_elicitation(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let request_id = params["requestId"].as_str().unwrap_or_default();
        let mut answer = json!({"action": params["action"].as_str().unwrap_or_default()});
        if let Some(content) = params.get("content").filter(|value| value.is_object()) {
            answer["content"] = content.clone();
        }
        let envelope = host_command::envelope(HostCommand::new(
            "resolveInteraction",
            Some(&target.task_id),
            json!({"interactionId": request_id, "answer": answer}),
        ))?;
        send_command(&context, &target, envelope, "resolveInteraction").await?;
        // 登记差异：web-remote-replayable 的本地 observer 事件由 V4 事件流收敛（Rust Host 暂无
        // 任务事件发射器）。
        Ok(Wire::Object(Value::Bool(true)))
    })
}

async fn read_session(context: &StateContext, target: &TaskTarget) -> Result<Value, RpcError> {
    let runtime = runtime(context)?;
    let answer = forward::runtime_call_shaped(
        &runtime.instances,
        &runtime.options,
        routing(target),
        Shaped {
            method: "session/read",
            body: json!({"sessionId": target.task_id}),
            with_workspace: false,
            trace: None,
        },
        Extract::Result,
    )
    .await?;
    match answer {
        Wire::Object(value) => Ok(value),
        _ => Err(RpcError::new("Fatal", "session/read returned a non-object")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_denylist_merges_automation_and_offpeak_rules() {
        assert_eq!(prompt_tool_denylist(&json!({})), Vec::<String>::new());
        let automation = prompt_tool_denylist(&json!({
            "automationId": "cron_1",
            "toolDenylist": ["Read", "CronCreate"],
        }));
        assert_eq!(
            automation,
            vec!["Read", "CronCreate", "CronUpdate", "CronDelete"]
        );
        let offpeak = prompt_tool_denylist(&json!({"offPeakTaskId": "off_1"}));
        assert_eq!(offpeak, vec!["OffPeakCreate"]);
        let both = prompt_tool_denylist(&json!({
            "automationId": "cron_1", "offPeakTaskId": "off_1",
        }));
        assert_eq!(
            both,
            vec!["CronCreate", "CronUpdate", "CronDelete", "OffPeakCreate"]
        );
        // cron 轮不 deny OffPeakCreate（与 TS 一致）。
        assert!(!automation.iter().any(|tool| tool == "OffPeakCreate"));
    }

    #[test]
    fn turn_attribution_prefers_automation() {
        assert_eq!(
            turn_attribution(&json!({"automationId": "a", "offPeakTaskId": "b"})),
            json!({"automationId": "a"})
        );
        assert_eq!(
            turn_attribution(&json!({"offPeakTaskId": "b", "offPeakRunType": "queued"})),
            json!({"offPeakTaskId": "b", "offPeakRunType": "queued"})
        );
        assert_eq!(turn_attribution(&json!({})), json!({}));
    }
}
