//! `source-server-management` 的 RPC 面：连接动作的参数与错误契约（S3-a，spec §10.10）。
//!
//! 这里不启动真实 transport：只验证方法已注册、参数按 `ProxyChannel` 数组布局解析、未知 profile
//! 显式报错，以及 `disconnectSource` 幂等返回快照。真实 transport 由
//! `apps/escode-host/tests/source_server_client_ts.rs` 用 TS fixture 覆盖。
mod support;
use escode_host_rpc::wire::Wire;
use serde_json::json;
use support::*;

async fn start_with_profile() -> (tempfile::TempDir, RpcSurface) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".escode").join("v2")).unwrap();
    std::fs::write(
        home.join(".escode").join("v2").join("setting.json"),
        serde_json::to_vec(&json!({
            "sourceServers": [{
                "sourceServerId": "33333333-3333-4333-8333-333333333333",
                "displayName": "fixture",
            }],
            "connectionProfiles": [{
                "profileId": "44444444-4444-4444-8444-444444444444",
                "sourceServerId": "33333333-3333-4333-8333-333333333333",
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "fixture", "username": "dev"},
                "createdAt": 1_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let surface = start(&home).await;
    (dir, surface)
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_profile_is_registered_and_rejects_unknown_profiles() {
    let (_dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        1,
        "source-server-management",
        "getSnapshot",
        &Wire::Array(vec![]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 1).await else {
        panic!("getSnapshot returns a snapshot object");
    };
    assert_eq!(snapshot["states"][0]["state"], json!("absent"));

    // 未知 profile：显式 RPC 错误，不写状态、不改 generation。
    call(
        &mut socket,
        2,
        "source-server-management",
        "connectProfile",
        &Wire::Array(vec![
            Wire::Str("55555555-5555-4555-8555-555555555555".to_owned()),
            Wire::Object(json!({})),
        ]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 2).await else {
        panic!("connectProfile failure returns PromiseError payload");
    };
    assert_eq!(error["name"], json!("Error"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown source server profile"))
    );

    call(
        &mut socket,
        3,
        "source-server-management",
        "getSnapshot",
        &Wire::Array(vec![]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 3).await else {
        panic!("getSnapshot returns a snapshot object");
    };
    assert_eq!(snapshot["generation"], json!(1));
    assert_eq!(snapshot["states"][0]["state"], json!("absent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_source_is_idempotent_and_returns_snapshot() {
    let (_dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    for (id, source) in [
        (1, "33333333-3333-4333-8333-333333333333"),
        (2, "66666666-6666-4666-8666-666666666666"),
    ] {
        call(
            &mut socket,
            id,
            "source-server-management",
            "disconnectSource",
            &Wire::Array(vec![Wire::Str(source.to_owned())]),
        )
        .await;
        let Wire::Object(snapshot) = read_response(&mut socket, id).await else {
            panic!("disconnectSource returns a snapshot object");
        };
        assert_eq!(snapshot["states"][0]["state"], json!("absent"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_projects_requires_connection_and_rejects_unknown_options() {
    let (_dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    for (id, arg) in [
        (10, Wire::Array(vec![])),
        (
            11,
            Wire::Array(vec![Wire::Object(
                json!({ "refreshMigrationCounts": true }),
            )]),
        ),
    ] {
        call(
            &mut socket,
            id,
            "source-server-management",
            "getSnapshot",
            &arg,
        )
        .await;
        let Wire::Object(snapshot) = read_response(&mut socket, id).await else {
            panic!("getSnapshot returns a snapshot object");
        };
        assert_eq!(
            snapshot["migrationCounts"][0]["status"],
            json!("requires-connection")
        );
        assert_eq!(snapshot["failedPathCount"], json!(0));
        assert_eq!(snapshot["abandonedPathCount"], json!(0));
    }

    for (id, arg) in [
        (
            12,
            Wire::Array(vec![Wire::Object(json!({ "unknown": true }))]),
        ),
        (
            13,
            Wire::Array(vec![Wire::Object(json!({
                "refreshMigrationCounts": "yes"
            }))]),
        ),
    ] {
        call(
            &mut socket,
            id,
            "source-server-management",
            "getSnapshot",
            &arg,
        )
        .await;
        let Wire::Object(error) = read_response(&mut socket, id).await else {
            panic!("invalid options return PromiseError payload");
        };
        assert_eq!(error["name"], json!("InvalidRequest"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn migration_methods_require_an_online_source() {
    const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
    let (_dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    for (id, method, arg) in [
        (
            20,
            "getMigrationReport",
            Wire::Array(vec![Wire::Str(SOURCE.to_owned())]),
        ),
        (
            21,
            "exportMigrationReport",
            Wire::Array(vec![Wire::Str(SOURCE.to_owned())]),
        ),
        (
            22,
            "abandonWorkspaceMigration",
            Wire::Array(vec![
                Wire::Str(SOURCE.to_owned()),
                Wire::Str("/remote/workspace".to_owned()),
                Wire::Str("migration-1".to_owned()),
            ]),
        ),
        (
            23,
            "retryWorkspaceMigration",
            Wire::Array(vec![
                Wire::Str(SOURCE.to_owned()),
                Wire::Str("/remote/workspace".to_owned()),
                Wire::Str("migration-1".to_owned()),
            ]),
        ),
    ] {
        call(&mut socket, id, "source-server-management", method, &arg).await;
        let Wire::Object(error) = read_response(&mut socket, id).await else {
            panic!("{method} must return PromiseError payload");
        };
        assert_eq!(error["name"], json!("Error"));
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains("must be connected")),
            "{method}: {error}"
        );
    }
}
