//! S4-j 连接层：身份变化待确认与确认后的 Profile 重绑定。

#![cfg(unix)]

use std::sync::Arc;

use serde_json::json;

use super::test_support::*;
use crate::credential::CredentialService;
use crate::settings;
use crate::source_server::{IdentityChangePending, ServerIdentity, SourceServerRegistry};
use crate::source_server_connections::{ConnectIntent, SourceServerConnections};

const BOUND_SOURCE: &str = "11111111-1111-4111-8111-111111111111";
const NEW_SOURCE: &str = "22222222-2222-4222-8222-222222222222";
const PROFILE: &str = "33333333-3333-4333-8333-333333333333";
const PASSWORD_KEY: &str = "connection-profile:old:password";

fn settings_json() -> serde_json::Value {
    json!({
        "sourceServers": [{
            "sourceServerId": BOUND_SOURCE,
            "displayName": "fixture",
            "preferredProfileId": PROFILE,
        }],
        "connectionProfiles": [{
            "profileId": PROFILE,
            "sourceServerId": BOUND_SOURCE,
            "displayName": "fixture ssh",
            "transport": "ssh",
            "target": {
                "kind": "ssh",
                "host": "fixture",
                "username": "dev",
                "passwordCredentialKey": PASSWORD_KEY,
            },
            "createdAt": 1_u64,
        }],
        "sourceServerWorkspaceHistory": [{
            "workspacePath": "/srv/work",
            "workspaceIdentity": "remote:ssh:fixture:22:dev:/srv/work",
            "profileId": PROFILE,
            "sourceServerId": BOUND_SOURCE,
            "lastOpenedAt": 5_u64,
        }],
    })
}

fn persistent_registry(dir: &tempfile::TempDir) -> (Arc<SourceServerRegistry>, std::path::PathBuf) {
    let home = dir.path().join("home");
    let path = settings::settings_path(&home);
    std::fs::create_dir_all(path.parent().expect("settings dir")).expect("dir");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&settings_json()).expect("json"),
    )
    .expect("settings");
    (Arc::new(SourceServerRegistry::load(&home)), home)
}

fn changed_identity() -> ServerIdentity {
    ServerIdentity {
        source_server_id: NEW_SOURCE.to_owned(),
        os_user: Some("root".to_owned()),
        hostname: Some("box-1".to_owned()),
        workspace_path: Some("/srv/work".to_owned()),
    }
}

async fn setup() -> (
    tempfile::TempDir,
    Arc<SourceServerRegistry>,
    SourceServerConnections,
    Arc<CredentialService>,
) {
    let dir = tempfile::tempdir().expect("temp");
    let (registry, home) = persistent_registry(&dir);
    let credentials = Arc::new(CredentialService::new(
        CredentialService::path(dir.path()),
        "test-secret",
    ));
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_identity(
        Arc::clone(&registry),
        runner,
        changed_identity(),
        Some(Arc::clone(&credentials)),
    );
    (dir, registry, connections, credentials)
}

#[tokio::test(flavor = "multi_thread")]
async fn unconfirmed_identity_change_is_pending_and_zero_write() {
    let (dir, registry, connections, _credentials) = setup().await;
    let path = settings::settings_path(&dir.path().join("home"));
    let before = std::fs::read(&path).expect("settings bytes");
    let generation = registry.generation();

    let error = connections
        .connect(PROFILE)
        .await
        .expect_err("identity change must require confirmation");
    let pending = error
        .downcast_ref::<IdentityChangePending>()
        .expect("pending error type");
    assert_eq!(pending.profile_id, PROFILE);
    assert_eq!(pending.pending_source_server_id, NEW_SOURCE);

    let snapshot = registry.snapshot().expect("snapshot");
    // bootstrapping 与 identity-change-pending 各发布一次状态。
    assert_eq!(snapshot["generation"], json!(generation + 2));
    let state = &snapshot["states"][0];
    assert_eq!(state["profileId"], json!(PROFILE));
    assert_eq!(state["state"], json!("identity-change-pending"));
    assert_eq!(state["pendingSourceServerId"], json!(NEW_SOURCE));
    assert_eq!(
        state["errorCode"],
        json!("source-server-identity-change-pending")
    );
    assert_eq!(state["retryable"], json!(false));
    assert_eq!(
        std::fs::read(&path).expect("settings bytes"),
        before,
        "未确认的身份变化不得写 settings"
    );
    assert!(connections.client(PROFILE).is_none());
    assert_eq!(connections.connected_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmed_identity_change_retires_old_profile_and_rekeys() {
    let (_dir, registry, connections, credentials) = setup().await;
    credentials
        .save(PASSWORD_KEY, "secret")
        .await
        .expect("save");

    connections
        .connect_with(PROFILE, ConnectIntent::ConfirmIdentityChange)
        .await
        .expect("confirmed connect");

    let snapshot = registry.snapshot().expect("snapshot");
    let profiles = snapshot["profiles"].as_array().expect("profiles");
    let old = profiles
        .iter()
        .find(|profile| profile["profileId"] == json!(PROFILE))
        .expect("old profile");
    assert_eq!(old["lifecycleState"], json!("retired"));
    assert_eq!(old["retiredReason"], json!("source-reinstalled"));
    let new = profiles
        .iter()
        .find(|profile| profile["sourceServerId"] == json!(NEW_SOURCE))
        .expect("new profile");
    assert_eq!(new["lifecycleState"], json!("active"));
    let new_profile_id = new["profileId"].as_str().expect("new profile id");
    assert_ne!(new_profile_id, PROFILE);
    assert_eq!(
        new["target"]["passwordCredentialKey"],
        json!(format!("connection-profile:{new_profile_id}:password"))
    );

    assert_eq!(
        credentials
            .load(&format!("connection-profile:{new_profile_id}:password"))
            .await
            .expect("load")
            .as_deref(),
        Some("secret")
    );
    let old_source = snapshot["servers"]
        .as_array()
        .expect("servers")
        .iter()
        .find(|server| server["sourceServerId"] == json!(BOUND_SOURCE))
        .expect("old source");
    assert_eq!(old_source["installState"], json!("uninstalled"));
    let new_source = snapshot["servers"]
        .as_array()
        .expect("servers")
        .iter()
        .find(|server| server["sourceServerId"] == json!(NEW_SOURCE))
        .expect("new source");
    assert_eq!(new_source["preferredProfileId"], json!(new_profile_id));

    let history = &snapshot["workspaceHistory"][0];
    assert_eq!(history["profileId"], json!(PROFILE));
    assert_eq!(history["sourceServerId"], json!(BOUND_SOURCE));
    assert_eq!(
        history["workspaceIdentity"],
        json!("remote:ssh:fixture:22:dev:/srv/work")
    );

    let states = snapshot["states"].as_array().expect("states");
    let old_state = states
        .iter()
        .find(|state| state["profileId"] == json!(PROFILE))
        .expect("old state");
    assert_eq!(old_state["state"], json!("offline"));
    let new_state = states
        .iter()
        .find(|state| state["profileId"] == json!(new_profile_id))
        .expect("new state");
    assert_eq!(new_state["state"], json!("online"));
    assert_eq!(connections.connected_count(), 1);
}
