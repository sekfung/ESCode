//! MCP client 的 `ClientHandler`：把 server 主动通知转成 runtime 事件（docs/specs/rust-mcp-client-events.md）。
//!
//! 连接层只做协议 → 纯数据的转换，不持有订阅或页面状态；订阅归属、实例凭证校验与 live-only
//! 投影由 Engine 的 `McpClientEvents` owner 处理。
// rmcp 3.5 按 SEP-2577 把 sampling 标为 deprecated；MCP Apps 仍依赖该回调，升级 SDK 前保留。
#![allow(deprecated)]
use std::future::Future;

use rmcp::{
    ClientHandler, ErrorData as McpError,
    model::{
        ClientConfig, CreateMessageRequestParams, CreateMessageResult,
        ResourceUpdatedNotificationParam,
    },
    service::{MaybeSendFuture, NotificationContext, RequestContext, RoleClient},
};

use crate::contract::{McpClientEvent, McpClientEventSink};

#[derive(Clone)]
pub(super) struct HubClient {
    info: ClientConfig,
    server: String,
    events: Option<McpClientEventSink>,
}

impl HubClient {
    pub(super) fn new(
        info: ClientConfig,
        server: &str,
        events: Option<McpClientEventSink>,
    ) -> Self {
        Self {
            info,
            server: server.to_owned(),
            events,
        }
    }

    /// 通知 → 事件出口；未登记出口时丢弃（不伪造订阅状态）。
    fn emit(&self, event: McpClientEvent) {
        if let Some(events) = &self.events {
            let _ = events.send(event);
        }
    }

    pub(super) fn resource_updated(&self, uri: &str) {
        self.emit(McpClientEvent::ResourceUpdated {
            server: self.server.clone(),
            uri: uri.to_owned(),
        });
    }

    pub(super) fn resource_list_changed(&self) {
        self.emit(McpClientEvent::ResourceListChanged {
            server: self.server.clone(),
        });
    }
}

impl ClientHandler for HubClient {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }

    /// server → client sampling：交给 Engine 用任务模型应答；未登记出口时显式失败。
    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, McpError>> + MaybeSendFuture + '_ {
        let events = self.events.clone();
        let server = self.server.clone();
        async move {
            let Some(events) = events else {
                return Err(McpError::internal_error(
                    "MCP sampling is unavailable in this runtime",
                    None,
                ));
            };
            let params = serde_json::to_value(params)
                .map_err(|error| McpError::internal_error(error.to_string(), None))?;
            let (reply, response) = tokio::sync::oneshot::channel();
            events
                .send(McpClientEvent::Sampling {
                    server,
                    params,
                    reply,
                })
                .map_err(|_| McpError::internal_error("MCP client event channel closed", None))?;
            match response.await {
                Ok(Ok(value)) => serde_json::from_value(value)
                    .map_err(|error| McpError::internal_error(error.to_string(), None)),
                Ok(Err(message)) => Err(McpError::internal_error(message, None)),
                Err(_) => Err(McpError::internal_error(
                    "MCP sampling request was dropped",
                    None,
                )),
            }
        }
    }

    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        let this = self.clone();
        async move { this.resource_updated(&params.uri) }
    }

    fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        let this = self.clone();
        async move { this.resource_list_changed() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(events: Option<McpClientEventSink>) -> HubClient {
        HubClient::new(ClientConfig::default(), "server-a", events)
    }

    #[test]
    fn routes_both_resource_notifications_to_the_sink() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let client = client(Some(tx));
        client.resource_updated("ui://app/index.html");
        client.resource_list_changed();
        assert!(matches!(
            rx.try_recv().expect("update"),
            McpClientEvent::ResourceUpdated { server, uri }
                if server == "server-a" && uri == "ui://app/index.html"
        ));
        assert!(matches!(
            rx.try_recv().expect("list"),
            McpClientEvent::ResourceListChanged { server } if server == "server-a"
        ));
    }

    #[test]
    fn drops_notifications_when_no_sink_is_registered() {
        let client = client(None);
        client.resource_updated("ui://app/index.html");
        client.resource_list_changed();
    }
}
