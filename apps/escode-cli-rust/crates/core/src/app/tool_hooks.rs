//! 工具边界的 hooks 调用点（TS tool/executor/hook-flow.ts + call-runner.ts，docs/specs/rust-hooks.md H1）：
//! PreToolUse 在预处理之后、权限之前；PostToolUse / PostToolUseFailure 在执行之后。执行与聚合由工作流宿主里的
//! TS hook 运行器完成，这里只负责调用时机与结果的模型面效果。
use super::context::TurnFacts;
use crate::contract::{ToolControl, ToolOutput, ToolPort};
use serde_json::{Value, json};

/// PreToolUse 的结论：追加上下文，以及交给权限判定的 allow / ask（TS applyPreToolPermissionDecision）。
#[derive(Default)]
pub(super) struct PreHook {
    pub contexts: Vec<String>,
    pub permission: Option<Value>,
}

fn input(turn: &TurnFacts, event: &str, call: &Value, args: &Value) -> Value {
    json!({
        "hookEventName": event, "mode": turn.mode, "sessionId": turn.mcp_meta["session_id"],
        "toolCallId": call["id"], "toolInput": args, "toolName": call["function"]["name"],
        "traceId": turn.mcp_meta["trace_id"], "turnId": turn.mcp_meta["turn_id"],
    })
}

fn arguments(call: &Value) -> Value {
    serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or(""))
        .unwrap_or(Value::Null)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// TS formatHookAdditionalContexts。
pub(super) fn format_contexts(contexts: &[String]) -> String {
    let mut parts = vec!["[Hook additional context]".to_owned()];
    parts.extend(
        contexts
            .iter()
            .enumerate()
            .map(|(i, c)| format!("#{}\n{c}", i + 1)),
    );
    parts.join("\n")
}

fn with_contexts(mut output: ToolOutput, contexts: &[String]) -> ToolOutput {
    if !contexts.is_empty() {
        output.content = format!("{}\n\n{}", output.content, format_contexts(contexts));
    }
    output
}

/// PreToolUse：拒绝 / 阻止继续时返回交回模型的失败输出；`updatedInput` 改写 `call` 的参数并重新校验。
pub(super) async fn pre_tool_use(
    tools: &dyn ToolPort,
    turn: &TurnFacts,
    call: &mut Value,
    definitions: &[Value],
) -> Result<PreHook, ToolOutput> {
    let session = turn.mcp_meta["session_id"].as_str().unwrap_or_default();
    let input = input(turn, "PreToolUse", call, &arguments(call));
    let Some(result) = tools.run_hook(session, input, call["id"].as_str()).await else {
        return Ok(PreHook::default());
    };
    let contexts = strings(&result["additionalContexts"]);
    if result["permissionBehavior"] == "deny" || result["preventContinuation"] == true {
        let reason = result["hookPermissionDecisionReason"]
            .as_str()
            .or(result["stopReason"].as_str())
            .unwrap_or("Blocked by PreToolUse hook");
        let output = ToolOutput {
            failed: true,
            control: ToolControl {
                denied: true,
                stop_turn: false,
            },
            ..ToolOutput::text(crate::domain::tool_failure::plain_error_text(reason))
        };
        return Err(with_contexts(output, &contexts));
    }
    if let Some(updated) = result.get("updatedInput").filter(|v| !v.is_null()) {
        call["function"]["arguments"] = updated.to_string().into();
        let name = call["function"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        match super::tool_dispatch::checked_input(definitions, &name, call) {
            Err(content) => {
                let output = ToolOutput {
                    failed: true,
                    ..ToolOutput::text(content)
                };
                return Err(with_contexts(output, &contexts));
            }
            Ok(Some(arguments)) => call["function"]["arguments"] = arguments.into(),
            Ok(None) => {}
        }
    }
    let permission = match result["permissionBehavior"].as_str() {
        Some(behavior @ ("allow" | "ask")) => Some(json!({
            "behavior": behavior, "reason": result["hookPermissionDecisionReason"],
        })),
        _ => None,
    };
    Ok(PreHook {
        contexts,
        permission,
    })
}

/// 执行之后：成功跑 PostToolUse、失败跑 PostToolUseFailure，把 PreToolUse 与之后的追加上下文接到模型可见内容后。
pub(super) async fn post_tool_use(
    tools: &dyn ToolPort,
    turn: &TurnFacts,
    call: &Value,
    pre: PreHook,
    output: ToolOutput,
    succeeded: bool,
) -> ToolOutput {
    if output.control.denied {
        return with_contexts(output, &pre.contexts);
    }
    let session = turn.mcp_meta["session_id"].as_str().unwrap_or_default();
    let mut input = input(
        turn,
        if succeeded {
            "PostToolUse"
        } else {
            "PostToolUseFailure"
        },
        call,
        &arguments(call),
    );
    if succeeded {
        let response = if output.data.is_null() {
            Value::from(output.content.clone())
        } else {
            output.data.clone()
        };
        input["toolResponse"] = response;
    } else {
        input["error"] = json!({"message": output.content, "type": "ToolExecutionFailed"});
        input["isInterrupt"] = false.into();
    }
    let mut contexts = pre.contexts;
    if let Some(result) = tools.run_hook(session, input, call["id"].as_str()).await {
        contexts.extend(strings(&result["additionalContexts"]));
    }
    with_contexts(output, &contexts)
}

/// TS applyPreToolPermissionDecision：hook allow 放行 ask（alwaysAsk 除外）；hook ask 把 allow 升级为确认。
/// 返回升级时的确认理由。
pub(super) fn merge_permission(
    decision: crate::domain::permission::Decision,
    hook: Option<&Value>,
    always_ask: bool,
) -> (crate::domain::permission::Decision, Option<String>) {
    use crate::domain::permission::{Behavior, Decision};
    let Some(hook) = hook else {
        return (decision, None);
    };
    let reason = hook["reason"].as_str().map(str::to_owned);
    match (hook["behavior"].as_str(), decision.behavior) {
        (Some("allow"), Behavior::Ask) if !always_ask => (
            Decision {
                behavior: Behavior::Allow,
                rule_id: "hook.PreToolUse.allow",
            },
            None,
        ),
        (Some("ask"), Behavior::Allow) => (
            Decision {
                behavior: Behavior::Ask,
                rule_id: "hook.PreToolUse.ask",
            },
            Some(reason.unwrap_or_else(|| "Tool requires approval by PreToolUse hook".into())),
        ),
        _ => (decision, None),
    }
}
