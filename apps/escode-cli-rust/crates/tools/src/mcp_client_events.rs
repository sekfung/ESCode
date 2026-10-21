//! MCP client 的 `ClientHandler`：把 server 主动通知转成 runtime 事件（docs/specs/rust-mcp-client-events.md）。
//!
//! 连接层只做协议 → 纯数据的转换，不持有订阅或页面状态；订阅归属、实例凭证校验与 live-only
//! 投影由 Engine 的 `McpClientEvents` owner 处理。
use std::future::Future;

use rmcp::{
    ClientHandler,
    model::{ClientConfig, ResourceUpdatedNotificationParam},
    service::{MaybeSendFuture, NotificationContext, RoleClient},
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
        assert_eq!(
            rx.try_recv().expect("update"),
            McpClientEvent::ResourceUpdated {
                server: "server-a".into(),
                uri: "ui://app/index.html".into()
            }
        );
        assert_eq!(
            rx.try_recv().expect("list"),
            McpClientEvent::ResourceListChanged {
                server: "server-a".into()
            }
        );
    }

    #[test]
    fn drops_notifications_when_no_sink_is_registered() {
        let client = client(None);
        client.resource_updated("ui://app/index.html");
        client.resource_list_changed();
    }
}
