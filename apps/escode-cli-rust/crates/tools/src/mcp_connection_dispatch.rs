//! `Connection` 的 JSON-RPC 分发与工具发现，拆分子模块以满足 400 行门禁。

use super::Connection;
use anyhow::{Context, Result, bail, ensure};
use rmcp::{model::ClientRequest, service::PeerRequestOptions};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio_util::sync::CancellationToken;

impl Connection {
    pub(super) async fn discover(&self, cancel: &CancellationToken) -> Result<Vec<Value>> {
        let mut tools = vec![];
        let mut cursor = Value::Null;
        let mut seen = BTreeSet::new();
        for _ in 0..128 {
            let params = if cursor.is_null() {
                json!({})
            } else {
                json!({"cursor":cursor})
            };
            let result = self.request("tools/list", params, cancel).await?;
            tools.extend(
                result["tools"]
                    .as_array()
                    .context("Invalid MCP tools list")?
                    .iter()
                    .cloned(),
            );
            ensure!(tools.len() <= 10_000, "MCP tool count exceeded");
            cursor = result["nextCursor"].clone();
            if cursor.is_null() {
                return Ok(tools);
            }
            ensure!(
                cursor.is_string() && seen.insert(cursor.to_string()),
                "MCP pagination loop"
            );
        }
        bail!("MCP tool pages exceeded")
    }
    pub(super) async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        Ok(serde_json::from_str(
            &self.request_text(method, params, cancel).await?,
        )?)
    }
    /// 应答的 JSON 文本（按 rmcp 类型的字段顺序），供需要保持键序的格式化使用。
    pub(super) async fn request_text(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let request: ClientRequest =
            serde_json::from_value(json!({"method":method,"params":params}))?;
        let handle = tokio::select! {biased;_=cancel.cancelled()=>bail!("Cancelled"),result=self.peer.send_cancellable_request(request,PeerRequestOptions::with_timeout(self.timeout))=>result.context("MCP request dispatch failed")?};
        let id = handle.id.clone();
        let result = tokio::select! {biased;
            _=cancel.cancelled()=>{
                let _=tokio::time::timeout(Duration::from_secs(2),self.peer.notify_cancelled(rmcp::model::CancelledNotificationParam::new(Some(id.clone()),Some("Cancelled".into())))).await;
                Err(anyhow::anyhow!("Cancelled"))
            },
            result=handle.await_response()=>result.map_err(|_|anyhow::anyhow!("MCP request failed or timed out")),
        };
        if result.is_ok() && method == "tools/list" {
            self.raw.register(&id);
        }
        if result.is_err() && method == "tools/call" {
            self.close().await?;
        }
        Ok(serde_json::to_string(&result?)?)
    }
}
