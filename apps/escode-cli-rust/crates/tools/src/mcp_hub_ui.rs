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
    Source, Subscriber, bound_for_ui, parse_binding, parse_lease, parse_scope, project_resources,
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

    /// 跨步骤操作（采样）开始：校验实例并登记 pin。
    pub fn ui_begin(&self, params: &Value) -> Result<()> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let operation = params["operationId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("operationId is required")?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        let mut ui = self.ui();
        ui.instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        let pin = ui.instances.retain(&lease, operation)?;
        ui.ops
            .insert((scope.session_id, lease.token, operation.to_owned()), pin);
        Ok(())
    }

    /// 跨步骤操作结束：释放 pin（幂等）。
    pub fn ui_end(&self, params: &Value) {
        let (Some(token), Some(operation)) = (
            params
                .get("instance")
                .and_then(|instance| instance["token"].as_str()),
            params["operationId"].as_str(),
        ) else {
            return;
        };
        let mut ui = self.ui();
        let key = (
            params["sessionId"].as_str().unwrap_or_default().to_owned(),
            token.to_owned(),
            operation.to_owned(),
        );
        if let Some(pin) = ui.ops.remove(&key) {
            ui.instances.release(&pin);
        }
    }

    /// 采样前后复用的实例校验（不改变状态）。
    pub fn ui_validate_scope(&self, params: &Value) -> Result<()> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        self.ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        Ok(())
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
            ui.subs
                .remove_instance(&scope.session_id, &lease.token, lease.generation);
            ui.ops.retain(|(session, token, _), _| {
                !(session == &scope.session_id && token == &lease.token)
            });
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

    /// `mcp/uiSubscribeResource` / `mcp/uiUnsubscribeResource`。
    ///
    /// 订阅者 = 会话 + 实例凭证 + 沙箱代际；同一 `(server, uri)` 只在第一位订阅时真正向
    /// MCP server 订阅，最后一位退订时才退订。
    pub async fn ui_subscribe(
        &self,
        params: &Value,
        subscribe: bool,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let scope = parse_scope(params)?;
        let lease = parse_lease(params)?;
        let uri = params["uri"].as_str().context("uri is required")?;
        let scope_id = params["scopeId"].as_str().context("scopeId is required")?;
        let generation = params["generation"]
            .as_u64()
            .context("generation is required")?;
        if scope_id != lease.token || generation != lease.generation {
            bail!("Invalid MCP App subscription binding");
        }
        let source = self.ui_source(&scope.session_id, &scope.server_name);
        let instance = self
            .ui()
            .instances
            .validate(&lease, &scope.as_scope(), source.as_ref())?;
        let connection = self
            .ui_connection(&scope.session_id, &scope.server_name)
            .context("MCP App connection is unavailable")?;
        let server = scope.server_name.clone();
        if subscribe {
            let first = self.ui().subs.for_resource(&server, uri).is_empty();
            if first {
                tokio::select! {biased;
                    _ = cancel.cancelled() => bail!("Cancelled"),
                    _ = instance.cancelled() => bail!("MCP App instance is no longer authorized"),
                    result = connection.subscribe_resource(uri, cancel) => result?,
                };
            }
            self.ui().subs.add(
                &server,
                uri,
                Subscriber {
                    session_id: scope.session_id.clone(),
                    plugin_id: scope.plugin_id.clone(),
                    lease,
                },
            );
        } else {
            let last = self.ui().subs.remove(
                &server,
                uri,
                &scope.session_id,
                &lease.token,
                lease.generation,
            );
            if last {
                tokio::select! {biased;
                    _ = cancel.cancelled() => bail!("Cancelled"),
                    result = connection.unsubscribe_resource(uri, cancel) => result?,
                };
            }
        }
        Ok(json!({}))
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

#[path = "mcp_hub_ui_notifications.rs"]
mod notifications;
