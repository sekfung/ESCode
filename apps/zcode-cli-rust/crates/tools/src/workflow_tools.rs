//! 工作流宿主负责的工具（CreateWorkflow / GetWorkflowRun …，docs/specs/rust-dynamic-workflow.md M1）：
//! 预处理与执行都转成宿主请求，宿主里跑的是 TS 原版的 validateInput → resolveInput → prepareApproval 与
//! handler。Rust 负责权限确认（`prepare_tool` 钩子）与会话；后台 run 的完成通知经 Host 通道回来。

use super::save_workflow::Prepared;
use super::workspace_tools::WorkspaceTools;
use crate::contract::ToolOutput;
use anyhow::Result;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub(crate) async fn prepare(
    tools: &WorkspaceTools,
    session: &str,
    name: &str,
    args: &Value,
    skill_loaded: bool,
) -> Result<Prepared> {
    let reply = tools
        .workflow_host
        .request(
            "tool.prepare",
            json!({
                "session": session,
                "cwd": tools.workspace_path.to_string_lossy(),
                "tool": name,
                "input": args,
                "skillLoaded": skill_loaded,
            }),
        )
        .await?;
    if let Some(message) = reply["rejected"].as_str() {
        return Ok(Err(message.to_owned()));
    }
    Ok(Ok((reply["input"].clone(), reply["ask"] == true)))
}

pub(crate) async fn execute(
    tools: &WorkspaceTools,
    session: &str,
    call_id: &str,
    name: &str,
    args: &Value,
    selection: &Value,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let request = tools.workflow_host.request(
        "tool.execute",
        json!({
            "selection": selection,
            "session": session,
            "cwd": tools.workspace_path.to_string_lossy(),
            "tool": name,
            "input": args,
            "callId": call_id,
        }),
    );
    let reply = tokio::select! {
        reply = request => reply?,
        _ = cancel.cancelled() => {
            let _ = tools
                .workflow_host
                .request("tool.cancel", json!({ "callId": call_id }))
                .await;
            anyhow::bail!("Cancelled");
        }
    };
    // TS formatModelContent 的结果：通常是字符串；其它形状（内容块数组）原样序列化。
    let content = match &reply["content"] {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Ok(ToolOutput::new(content, reply["data"].clone()))
}

/// actor 会话里由宿主裁决的工具（TS driver 的 submit / escalate 端口，docs/specs/rust-dynamic-workflow.md M2）。
pub(crate) const ACTOR_TOOLS: [&str; 2] = ["submit_result", "escalate"];

/// 在宿主里执行 actor 会话的 `submit_result` / `escalate`：宿主跑 TS 原版 handler（阻塞在 driver 的裁决上），
/// 被拒的提交是可修复的工具失败（`<tool_use_error>`，同一轮里重试），accept 时停下本轮。
pub(crate) async fn execute_actor_tool(
    tools: &WorkspaceTools,
    session: &str,
    call_id: &str,
    name: &str,
    args: &Value,
) -> Result<ToolOutput> {
    let reply = tools
        .workflow_host
        .request(
            "actor.tool",
            json!({ "actorSession": session, "callId": call_id, "tool": name, "input": args }),
        )
        .await?;
    let text = reply["content"].as_str().unwrap_or_default().to_owned();
    let failed = reply["isError"] == true;
    let content = if reply["handlerFailure"] == true {
        format!("<tool_use_error>{text}</tool_use_error>")
    } else {
        text
    };
    let mut output = ToolOutput::new(content, Value::Null);
    output.failed = failed;
    output.control.stop_turn = reply["stopTurn"] == true;
    Ok(output)
}
