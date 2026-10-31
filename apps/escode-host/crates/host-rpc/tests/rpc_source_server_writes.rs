//! `source-server-management` 写面 RPC（docs/specs/source-server-remote-rust-host.md §9.8）：
//! 参数布局、成功返回新快照、settings 落盘与幂等空操作。
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

fn settings_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path()
        .join("home")
        .join(".escode")
        .join("v2")
        .join("setting.json")
}

fn stored_settings(dir: &tempfile::TempDir) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(settings_file(dir)).expect("settings file"))
        .expect("json")
}

#[tokio::test(flavor = "multi_thread")]
async fn management_writes_return_fresh_snapshots_and_persist() {
    const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
    const PROFILE: &str = "44444444-4444-4444-8444-444444444444";
    let (dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        30,
        "source-server-management",
        "renameSource",
        &Wire::Array(vec![
            Wire::Str(SOURCE.to_owned()),
            Wire::Str("renamed".to_owned()),
        ]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 30).await else {
        panic!("renameSource returns a snapshot");
    };
    assert_eq!(snapshot["servers"][0]["displayName"], json!("renamed"));
    assert_eq!(snapshot["generation"], json!(2));

    call(
        &mut socket,
        31,
        "source-server-management",
        "renameProfile",
        &Wire::Array(vec![
            Wire::Str(PROFILE.to_owned()),
            Wire::Str("conn".to_owned()),
        ]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 31).await else {
        panic!("renameProfile returns a snapshot");
    };
    assert_eq!(snapshot["profiles"][0]["displayName"], json!("conn"));

    call(
        &mut socket,
        32,
        "source-server-management",
        "setPreferredProfile",
        &Wire::Array(vec![
            Wire::Str(SOURCE.to_owned()),
            Wire::Str(PROFILE.to_owned()),
        ]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 32).await else {
        panic!("setPreferredProfile returns a snapshot");
    };
    assert_eq!(snapshot["servers"][0]["preferredProfileId"], json!(PROFILE));

    call(
        &mut socket,
        33,
        "source-server-management",
        "removeProfile",
        &Wire::Array(vec![Wire::Str(PROFILE.to_owned())]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 33).await else {
        panic!("removeProfile returns a snapshot");
    };
    assert_eq!(snapshot["profiles"], json!([]));
    // 1 起始；renameSource/renameProfile/setPreferredProfile 各 +1；removeProfile 先
    // disconnect 发布 absent 状态 +1，再落 settings 发布会目录变更 +1。
    assert_eq!(snapshot["generation"], json!(6));

    let stored = stored_settings(&dir);
    assert_eq!(stored["sourceServers"][0]["displayName"], json!("renamed"));
    assert_eq!(stored["connectionProfiles"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn management_writes_reject_invalid_input_without_generation_changes() {
    let (dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        40,
        "source-server-management",
        "renameSource",
        &Wire::Array(vec![
            Wire::Str("33333333-3333-4333-8333-333333333333".to_owned()),
            Wire::Str("   ".to_owned()),
        ]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 40).await else {
        panic!("empty rename returns an error payload");
    };
    assert_eq!(error["name"], json!("Error"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("display name is required"))
    );

    call(
        &mut socket,
        41,
        "source-server-management",
        "updateProfile",
        &Wire::Array(vec![Wire::Object(json!({
            "profileId": "44444444-4444-4444-8444-444444444444",
            "profileDisplayName": "conn",
            "target": {"kind": "server"},
        }))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 41).await else {
        panic!("invalid target returns an error payload");
    };
    assert_eq!(error["name"], json!("Error"));

    call(
        &mut socket,
        42,
        "source-server-management",
        "removeSourceServer",
        &Wire::Array(vec![Wire::Str(
            "77777777-7777-4777-8777-777777777777".to_owned(),
        )]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 42).await else {
        panic!("unknown source removal returns a snapshot");
    };
    assert_eq!(snapshot["generation"], json!(1));

    call(
        &mut socket,
        43,
        "source-server-management",
        "getSnapshot",
        &Wire::Array(vec![]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 43).await else {
        panic!("getSnapshot returns a snapshot");
    };
    assert_eq!(snapshot["generation"], json!(1));
    assert_eq!(snapshot["servers"][0]["displayName"], json!("fixture"));
    assert_eq!(
        stored_settings(&dir)["connectionProfiles"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn conversation_workspace_and_identity_change_require_a_known_profile() {
    let (_dir, surface) = start_with_profile().await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        50,
        "source-server-management",
        "ensureConversationWorkspace",
        &Wire::Array(vec![Wire::Str(
            "33333333-3333-4333-8333-333333333333".to_owned(),
        )]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 50).await else {
        panic!("offline source returns an error payload");
    };
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("must be connected"))
    );

    call(
        &mut socket,
        51,
        "source-server-management",
        "confirmIdentityChange",
        &Wire::Array(vec![Wire::Str(
            "88888888-8888-4888-8888-888888888888".to_owned(),
        )]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 51).await else {
        panic!("unknown profile returns an error payload");
    };
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown source server profile"))
    );
}
