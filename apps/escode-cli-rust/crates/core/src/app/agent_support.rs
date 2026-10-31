//! Agent 循环的收尾等待与小判定（从 agent_loop.rs 拆出）。

use crate::contract::ToolPort;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) fn safe(tools: &dyn ToolPort, session: &str, call: &Value) -> bool {
    let name = call["function"]["name"].as_str().unwrap_or("");
    tools.concurrent_safe_scoped(session, name) && !tools.requires_permission(name)
}

/// 成功 ToolSearch 结果里的命中工具名；local 结果没有 `tool_reference`，命中名随结果消息
/// 入史后冷恢复才能重建已发现集合（rust-tool-search.md §5.1）。
pub(super) fn tool_search_matches(tool: &str, failed: bool, data: &Value) -> Vec<String> {
    if tool != crate::domain::tool_search_catalog::TOOL_SEARCH_NAME || failed {
        return Vec::new();
    }
    data["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// 工具结果入史消息：含媒体或 tool_reference 的结果以 part 数组入史，由 model 协议层按 API
/// 格式投影（rust-media-read.md、rust-tool-search.md）；成功 ToolSearch 的命中名一并写入。
pub(super) fn tool_result_message(
    id: String,
    tool: String,
    content: &str,
    media: &[Value],
    failed: bool,
    model_call_wrapper: Option<&str>,
    tool_search_matches: &[String],
) -> Value {
    let body = if media.is_empty() {
        Value::String(content.to_owned())
    } else {
        Value::Array(media.to_vec())
    };
    let mut message = json!({"role":"tool","tool_call_id":id,"content":body,"_escode_tool_failed":failed,"_escode_tool_name":tool});
    if let Some(wrapper) = model_call_wrapper {
        message["_escode_model_call_wrapper"] = wrapper.into();
    }
    crate::domain::tool_search_announcement::annotate_discovered(&mut message, tool_search_matches);
    message
}

pub(super) async fn durable(
    receipt: oneshot::Receiver<()>,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::select! {biased;
        _=cancel.cancelled()=>bail!("Cancelled"),
        result=receipt=>result.context("Session owner stopped before durable commit"),
    }
}
