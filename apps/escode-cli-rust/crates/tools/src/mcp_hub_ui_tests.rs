//! Hub 侧通知路由与订阅清理（S2 验收：实例撤销后通知丢弃、会话清理）。
use super::Hub;
use crate::contract::McpClientEvent;
use crate::mcp_ui::{Binding, Source, Subscriber};
use serde_json::json;

fn binding() -> Binding {
    Binding {
        workspace: "ws".into(),
        account_context: None,
        session_id: "session".into(),
        plugin_id: "plugin".into(),
        server_name: "server".into(),
        scope_id: "scope".into(),
        resource_uri: "ui://a".into(),
        owner_web_contents_id: 1,
    }
}

fn hub_with_subscription() -> (Hub, Source) {
    let hub = Hub::new(std::path::PathBuf::from("/tmp"));
    let source = Source {
        identity: "key".into(),
        generation: 1,
    };
    hub.state_mut().sources.insert(
        "session".into(),
        [("server".to_owned(), source.clone())].into(),
    );
    let lease = hub.ui().instances.open(binding(), source.clone());
    hub.ui().subs.add(
        "server",
        "ui://a",
        Subscriber {
            session_id: "session".into(),
            plugin_id: "plugin".into(),
            lease,
        },
    );
    (hub, source)
}

#[test]
fn resource_update_routes_one_live_delta_per_session() {
    let (hub, _) = hub_with_subscription();
    let deltas = hub.ui_notifications(&McpClientEvent::ResourceUpdated {
        server: "server".into(),
        uri: "ui://a".into(),
    });
    assert_eq!(deltas.len(), 1);
    let (session, delta) = &deltas[0];
    assert_eq!(session, "session");
    assert_eq!(delta["op"], "pluginUi.resourceUpdated");
    assert_eq!(delta["serverName"], "server");
    assert_eq!(delta["uri"], "ui://a");
    // TS 口径：subscribers.scopeId 就是实例 token（沙箱作用域 id），不是清单里的 scopeId。
    assert_eq!(
        delta["subscribers"][0]["scopeId"],
        delta["subscribers"][0]["instance"]["token"]
    );
}

#[test]
fn stale_connection_drops_notifications_and_removes_the_subscription() {
    let (hub, _) = hub_with_subscription();
    // 模拟重连：身份不变、代际前进。
    hub.state_mut().sources.insert(
        "session".into(),
        [(
            "server".to_owned(),
            Source {
                identity: "key".into(),
                generation: 2,
            },
        )]
        .into(),
    );
    let deltas = hub.ui_notifications(&McpClientEvent::ResourceUpdated {
        server: "server".into(),
        uri: "ui://a".into(),
    });
    assert!(deltas.is_empty(), "旧代际的订阅不得投递");
    assert!(
        hub.ui().subs.for_resource("server", "ui://a").is_empty(),
        "失效订阅必须移除"
    );
}

#[test]
fn resource_list_changed_groups_all_resources_of_the_server() {
    let (hub, _) = hub_with_subscription();
    let lease = hub.ui().instances.open(
        binding(),
        Source {
            identity: "key".into(),
            generation: 1,
        },
    );
    hub.ui().subs.add(
        "server",
        "ui://b",
        Subscriber {
            session_id: "session".into(),
            plugin_id: "plugin".into(),
            lease,
        },
    );
    let deltas = hub.ui_notifications(&McpClientEvent::ResourceListChanged {
        server: "server".into(),
    });
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].1["op"], "pluginUi.resourceListChanged");
    assert_eq!(
        deltas[0].1["subscribers"].as_array().unwrap().len(),
        1,
        "同一实例的多个资源按 scopeId 去重"
    );
}

#[test]
fn notification_without_subscribers_is_dropped() {
    let hub = Hub::new(std::path::PathBuf::from("/tmp"));
    assert!(
        hub.ui_notifications(&McpClientEvent::ResourceUpdated {
            server: "server".into(),
            uri: "ui://none".into(),
        })
        .is_empty()
    );
    let _ = json!({});
}
