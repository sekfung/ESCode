//! MCP 客户端通知 → live-only delta（`pluginUi.resourceUpdated` 等），拆分子模块以满足 400 行门禁。

use super::Hub;
use crate::mcp_ui::Subscriber;
use serde_json::{Value, json};

impl Hub {
    /// MCP server 通知 → 每会话一条 live-only delta（`pluginUi.resourceUpdated` 等）。
    ///
    /// 凭证失效的订阅者就地移除并丢弃；没有有效订阅者时返回空。
    pub(crate) fn ui_notifications(
        &self,
        event: &crate::contract::McpClientEvent,
    ) -> Vec<(String, Value)> {
        let (server, updated_uri) = match event {
            crate::contract::McpClientEvent::ResourceUpdated { server, uri } => {
                (server.as_str(), Some(uri.as_str()))
            }
            crate::contract::McpClientEvent::ResourceListChanged { server } => {
                (server.as_str(), None)
            }
            crate::contract::McpClientEvent::Sampling { .. } => return Vec::new(),
        };
        let entries: Vec<(Option<String>, Subscriber)> = match updated_uri {
            Some(uri) => self
                .ui()
                .subs
                .for_resource(server, uri)
                .into_iter()
                .map(|subscriber| (Some(uri.to_owned()), subscriber))
                .collect(),
            None => self
                .ui()
                .subs
                .for_server(server)
                .into_iter()
                .map(|(uri, subscriber)| (Some(uri), subscriber))
                .collect(),
        };
        let mut groups: std::collections::BTreeMap<(String, String), Vec<Value>> =
            std::collections::BTreeMap::new();
        for (uri, subscriber) in entries {
            let source = self.ui_source(&subscriber.session_id, server);
            let scope = crate::mcp_ui::Scope {
                workspace: None,
                session_id: &subscriber.session_id,
                plugin_id: &subscriber.plugin_id,
                server_name: server,
            };
            let mut ui = self.ui();
            if ui
                .instances
                .validate(&subscriber.lease, &scope, source.as_ref())
                .is_err()
            {
                if let Some(uri) = &uri {
                    ui.subs.remove(
                        server,
                        uri,
                        &subscriber.session_id,
                        &subscriber.lease.token,
                        subscriber.lease.generation,
                    );
                }
                continue;
            }
            let item = json!({
                "scopeId": subscriber.lease.token,
                "generation": subscriber.lease.generation,
                "instance": subscriber.lease.to_value(),
            });
            let group = groups
                .entry((subscriber.session_id, subscriber.plugin_id))
                .or_default();
            // `list_changed` 会把同一实例在同一 server 的多个资源都列出来，按 scopeId 去重。
            if !group
                .iter()
                .any(|existing| existing["scopeId"] == item["scopeId"])
            {
                group.push(item);
            }
        }
        groups
            .into_iter()
            .map(|((session_id, plugin_id), subscribers)| {
                let mut delta = json!({
                    "op": if updated_uri.is_some() {
                        "pluginUi.resourceUpdated"
                    } else {
                        "pluginUi.resourceListChanged"
                    },
                    "pluginId": plugin_id,
                    "serverName": server,
                    "subscribers": subscribers,
                });
                if let Some(uri) = updated_uri {
                    delta["uri"] = json!(uri);
                }
                (session_id, delta)
            })
            .collect()
    }
}
