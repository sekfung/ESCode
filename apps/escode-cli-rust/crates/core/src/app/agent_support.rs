//! Agent 循环的收尾等待与小判定（从 agent_loop.rs 拆出）。

use crate::contract::ToolPort;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) fn safe(tools: &dyn ToolPort, session: &str, call: &Value) -> bool {
    let name = call["function"]["name"].as_str().unwrap_or("");
    tools.concurrent_safe_scoped(session, name) && !tools.requires_permission(name)
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
