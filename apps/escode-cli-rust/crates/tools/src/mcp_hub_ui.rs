//! MCP Apps 的 Hub 侧实现（docs/specs/rust-mcp-ui.md P1/P2）。
//!
//! 归属真相是本会话已连接的 MCP server；实例凭证、调用登记与资源边界都在 `crate::mcp_ui`。
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::Hub;
use crate::mcp_connection::Connection;
use crate::mcp_ui::{
    Source, bound_for_ui, parse_binding, parse_lease, parse_scope, project_resources,
    project_templates,
};

impl Hub {
    /// 当前会话里某个 server 的连接来源；未连接/已断开时返回 None。
    pub(super) fn ui_source(&self, session: &str, server: &str) -> Option<Source> {
        self.state()
            .sources
            .get(session)
            .and_then(|sources| sources.get(server))
            .cloned()
    }

    fn ui_connection(&self, session: &str, server: &str) -> Option<Arc<Connection>> {
        let state = self.state();
        let source = state.sources.get(session)?.get(server)?;
        state.connections.get(&source.identity).cloned()
    }

    pub(super) fn ui(&self) -> std::sync::MutexGuard<'_, crate::mcp_ui::McpUi> {
        self.ui
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `mcp/uiOpenInstance`：签发或复用实例凭证。
    pub async fn ui_open(&self, params: &Value) -> Result<Value> {
        let binding = parse_binding(params)?;
        let source = self
            .ui_source(&binding.session_id, &binding.server_name)
            .context("MCP App connection is unavailable")?;
        let lease = self.ui().instances.open(binding, source);
        Ok(lease.to_value())
    }

    /// `mcp/uiValidateInstance`。
    pub async fn ui_validate(&self, params: &Value) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        self.ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        Ok(json!({}))
    }

    /// `mcp/uiCloseInstance`；`onlyIfIdle` 与调用 pin 原子裁决。
    pub async fn ui_close(&self, params: &Value) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let mut ui = self.ui();
        if params["onlyIfIdle"] == true && ui.instances.is_busy(&lease) {
            return Ok(json!({"closed": false}));
        }
        let closed = ui.instances.close_in_scope(&lease, &scope.as_scope());
        if closed {
            ui.calls.abort_instance(&scope.session_id, &lease.token);
        }
        drop(ui);
        Ok(json!({"closed": true}))
    }

    /// `mcp/readResource`（Host 取 ui:// HTML，保留 `_meta`）。
    pub async fn ui_read_host_resource(
        &self,
        params: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.ui_read(params, cancel, false).await
    }

    /// `mcp/uiReadResource`（页面读资源，去 `_meta` 并套大小/MIME 边界）。
    pub async fn ui_read_page_resource(
        &self,
        params: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.ui_read(params, cancel, true).await
    }

    async fn ui_read(
        &self,
        params: &Value,
        cancel: &CancellationToken,
        for_ui: bool,
    ) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let uri = params["uri"].as_str().context("uri is required")?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        let instance = self
            .ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        if !for_ui {
            let bound = self
                .ui()
                .instances
                .resource_uri(&lease)
                .context("MCP App instance is no longer authorized")?;
            if bound != uri {
                bail!("MCP App resource binding mismatch");
            }
        }
        let connection = self
            .ui_connection(&scope.session_id, &scope.server_name)
            .context("MCP App connection is unavailable")?;
        let raw = tokio::select! {biased;
            _ = cancel.cancelled() => bail!("Cancelled"),
            _ = instance.cancelled() => bail!("MCP App instance is no longer authorized"),
            result = connection.read_resource(uri, cancel) => result?,
        };
        if for_ui {
            bound_for_ui(uri, &raw)
        } else {
            Ok(raw)
        }
    }

    /// `mcp/uiListResources` / `mcp/uiListResourceTemplates`。
    pub async fn ui_list_resources(
        &self,
        params: &Value,
        templates: bool,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        let instance = self
            .ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        let connection = self
            .ui_connection(&scope.session_id, &scope.server_name)
            .context("MCP App connection is unavailable")?;
        let cursor = params["cursor"].as_str();
        let raw = tokio::select! {biased;
            _ = cancel.cancelled() => bail!("Cancelled"),
            _ = instance.cancelled() => bail!("MCP App instance is no longer authorized"),
            result = async {
                if templates {
                    connection.list_resource_templates(cursor, cancel).await
                } else {
                    connection.list_resources(cursor, cancel).await
                }
            } => result?,
        };
        Ok(if templates {
            project_templates(&raw)
        } else {
            project_resources(&raw)
        })
    }

    /// `mcp/uiCallTool`：归属 + visibility 校验后复用 MCP 连接，不产生 transcript row。
    pub async fn ui_call_tool(&self, params: &Value, cancel: &CancellationToken) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let tool = params["toolName"]
            .as_str()
            .context("toolName is required")?;
        let call_id = params["callId"].as_str().context("callId is required")?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        let instance = self
            .ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        let connection = self
            .ui_connection(&scope.session_id, &scope.server_name)
            .context("MCP App connection is unavailable")?;
        let visibility = connection
            .tools
            .iter()
            .find(|candidate| candidate["name"].as_str() == Some(tool))
            .map(visibility)
            .with_context(|| format!("MCP tool \"{tool}\" is not available"))?;
        if !visibility.contains(&"app") {
            bail!("MCP tool \"{tool}\" is not visible to plugin UI");
        }
        let call_cancel = self
            .ui()
            .calls
            .begin(&scope.session_id, &lease.token, call_id)?;
        let pin = self.ui().instances.retain(&lease, call_id)?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let meta = json!({});
        let result = tokio::select! {biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("Cancelled")),
            _ = instance.cancelled() => Err(anyhow::anyhow!("MCP App instance is no longer authorized")),
            _ = call_cancel.cancelled() => Err(anyhow::anyhow!("Cancelled")),
            result = connection.call(tool, &args, &meta, None, cancel) => result,
        };
        {
            let mut ui = self.ui();
            ui.calls.finish(&scope.session_id, &lease.token, call_id);
            ui.instances.release(&pin);
        }
        Ok(result?.data)
    }

    /// `mcp/uiCancelCall`。
    pub async fn ui_cancel_call(&self, params: &Value) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let call_id = params["callId"].as_str().context("callId is required")?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        self.ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        let cancelled = self
            .ui()
            .calls
            .cancel(&scope.session_id, &lease.token, call_id);
        Ok(json!({"cancelled": cancelled}))
    }
}

/// `_meta.ui.visibility` 归一化；缺省 `["model","app"]`（TS `resolveMcpToolVisibility`）。
fn visibility(tool: &Value) -> Vec<&str> {
    tool["_meta"]["ui"]["visibility"]
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_else(|| vec!["model", "app"])
}
