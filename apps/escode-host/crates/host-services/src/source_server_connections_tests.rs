use std::time::Duration;

use anyhow::bail;
use serde_json::{Value, json};

use super::*;

fn registry() -> Arc<SourceServerRegistry> {
    Arc::new(
        SourceServerRegistry::from_settings(&json!({
            "sourceServers": [{
                "sourceServerId": "33333333-3333-4333-8333-333333333333",
                "displayName": "build-box",
            }],
            "connectionProfiles": [{
                "profileId": "44444444-4444-4444-8444-444444444444",
                "sourceServerId": "33333333-3333-4333-8333-333333333333",
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "build.example", "username": "dev"},
                "createdAt": 1_700_000_000_000_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("registry"),
    )
}

fn options(failure: &'static str) -> ConnectionsOptions {
    ConnectionsOptions::new("3.15.2", move |_profile| {
        Box::pin(async move { bail!("{failure}") })
    })
}

async fn next_state(
    receiver: &mut tokio::sync::broadcast::Receiver<crate::source_server::SourceServerEvent>,
) -> Value {
    loop {
        // 状态与操作日志共用一条事件总线：这里只取状态，其余事件由各自订阅者消费。
        match tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("event within timeout")
            .expect("event")
        {
            crate::source_server::SourceServerEvent::State(value) => return value,
            _ => continue,
        }
    }
}

/// 未知 profile 显式报错，且不产生任何状态（不创建空状态、不改 generation）。
#[tokio::test]
async fn unknown_profile_fails_without_state() {
    let registry = registry();
    let connections = SourceServerConnections::new(registry.clone(), options("must not resolve"));
    let mut events = registry.subscribe();
    let error = connections
        .connect("55555555-5555-4555-8555-555555555555")
        .await
        .expect_err("unknown profile must fail");
    assert!(format!("{error:#}").contains("unknown source server profile"));
    assert_eq!(registry.generation(), 1);
    assert!(
        events.try_recv().is_err(),
        "no state event for unknown profile"
    );
    assert!(
        connections
            .client("55555555-5555-4555-8555-555555555555")
            .is_none()
    );
}

/// transport 解析失败：先 `bootstrapping` 后 `failed`，错误与 generation 单调递增。
#[tokio::test]
async fn transport_resolution_failure_marks_failed() {
    let registry = registry();
    let connections = SourceServerConnections::new(registry.clone(), options("no ssh binary"));
    let mut events = registry.subscribe();
    let error = connections
        .connect("44444444-4444-4444-8444-444444444444")
        .await
        .expect_err("transport failure must surface");
    assert!(format!("{error:#}").contains("no ssh binary"));

    let bootstrapping = next_state(&mut events).await;
    assert_eq!(bootstrapping["generation"], json!(2));
    assert_eq!(bootstrapping["state"]["state"], json!("bootstrapping"));
    assert_eq!(
        bootstrapping["state"]["profileId"],
        json!("44444444-4444-4444-8444-444444444444")
    );

    let failed = next_state(&mut events).await;
    assert_eq!(failed["generation"], json!(3));
    assert_eq!(failed["state"]["state"], json!("failed"));
    assert_eq!(failed["state"]["retryable"], json!(true));
    assert!(
        failed["state"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("no ssh binary"))
    );
    assert!(
        failed["state"].get("errorCode").is_none(),
        "无法归因的错误不得编造 errorCode"
    );

    let snapshot = registry.snapshot().expect("snapshot");
    assert_eq!(snapshot["generation"], json!(3));
    assert_eq!(snapshot["states"][0]["state"], json!("failed"));
}

/// 显式收口：unknown Source 幂等成功；已知 profile 在没有连接时写 `absent`。
#[tokio::test]
async fn disconnect_is_idempotent_and_marks_absent() {
    let registry = registry();
    let connections = SourceServerConnections::new(registry.clone(), options("unused"));
    connections
        .disconnect_source("66666666-6666-4666-8666-666666666666")
        .await;
    assert_eq!(registry.generation(), 1, "未知 Source 不写状态");

    connections
        .disconnect("44444444-4444-4444-8444-444444444444")
        .await;
    let snapshot = registry.snapshot().expect("snapshot");
    assert_eq!(snapshot["states"][0]["state"], json!("absent"));
    assert_eq!(snapshot["generation"], json!(2));
}

/// 运行态状态覆盖 settings 派生的 `failed` 投影（同一 profile，单一状态源）。
#[test]
fn runtime_state_overrides_settings_projection() {
    let registry = Arc::new(
        SourceServerRegistry::from_settings(&json!({
            "connectionProfiles": [{
                "profileId": "77777777-7777-4777-8777-777777777777",
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "h", "username": "u"},
                "createdAt": 1_u64,
                "lastConnectedAt": 9_u64,
                "lastConnectError": "previous failure",
            }],
        }))
        .expect("registry"),
    );
    let profile_id = "77777777-7777-4777-8777-777777777777";
    assert_eq!(
        registry.snapshot().expect("snapshot")["states"][0]["state"],
        json!("failed")
    );
    let mut state = ConnectionStateSnapshot::new(profile_id, None, "online", 42);
    state.retryable = Some(true);
    state.current_version = Some("3.15.2".to_owned());
    registry.set_connection_state(state);
    let snapshot = registry.snapshot().expect("snapshot");
    assert_eq!(snapshot["generation"], json!(2));
    assert_eq!(snapshot["states"][0]["state"], json!("online"));
    assert_eq!(snapshot["states"][0]["currentVersion"], json!("3.15.2"));
    assert!(snapshot["states"][0].get("error").is_none());
}
