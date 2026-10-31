//! S4-j handshake record 单测。

use serde_json::{Value, json};

use super::*;
use crate::credential::CredentialService;
use crate::settings;
use crate::source_server::SourceServerRegistry;

const SOURCE_A: &str = "11111111-1111-4111-8111-111111111111";
const SOURCE_B: &str = "22222222-2222-4222-8222-222222222222";
const PROFILE: &str = "33333333-3333-4333-8333-333333333333";
const PASSWORD_KEY: &str = "connection-profile:PROFILE:password";

fn fixtures(bound_source: Option<&str>) -> Value {
    json!({
        "sourceServers": [{"sourceServerId": SOURCE_A, "displayName": "box", "preferredProfileId": PROFILE}],
        "connectionProfiles": [{
            "profileId": PROFILE,
            "sourceServerId": bound_source,
            "transport": "ssh",
            "target": {
                "kind": "ssh",
                "host": "a.example",
                "username": "dev",
                "sshConfigAlias": "prod-box",
                "passwordCredentialKey": PASSWORD_KEY,
            },
            "createdAt": 1,
        }],
        "sourceServerWorkspaceHistory": [{
            "workspacePath": "/srv/work",
            "workspaceIdentity": "remote:ssh:a.example:22:dev:/srv/work",
            "legacyWorkspaceIdentity": "remote:ssh:a.example:22:dev:/srv/work",
            "profileId": PROFILE,
            "sourceServerId": SOURCE_A,
            "lastOpenedAt": 5,
        }],
        "unrelated": {"keep": true},
    })
}

async fn registry_with(
    value: Value,
) -> (tempfile::TempDir, SourceServerRegistry, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("dir");
    let home = dir.path().join("home");
    let path = settings::settings_path(&home);
    std::fs::create_dir_all(path.parent().unwrap()).expect("settings dir");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).expect("settings");
    let registry = SourceServerRegistry::load(&home);
    (dir, registry, home)
}

fn credentials(dir: &tempfile::TempDir) -> CredentialService {
    CredentialService::new(CredentialService::path(dir.path()), "test-secret")
}

fn record(source_server_id: &str) -> HandshakeRecord {
    HandshakeRecord {
        source_server_id: source_server_id.to_owned(),
        os_user: Some("root".to_owned()),
        hostname: Some("box-1".to_owned()),
        version: "3.16.2".to_owned(),
        connected_at: 100,
        workspaces: vec![DeclaredWorkspaceInput {
            path: "/srv/work".to_owned(),
            workspace_identity: None,
            label: None,
        }],
    }
}

#[tokio::test]
async fn first_handshake_binds_source_and_rewrites_history() {
    let (dir, registry, home) = registry_with(fixtures(None)).await;
    let service = credentials(&dir);
    service.save(PASSWORD_KEY, "secret").await.unwrap();
    let outcome = registry
        .record_handshake(PROFILE, &record(SOURCE_B), false, Some(&service))
        .await
        .expect("handshake");
    assert!(!outcome.identity_changed);
    assert_eq!(
        outcome.effective_profile.source_server_id.as_deref(),
        Some(SOURCE_B)
    );
    let snapshot = registry.snapshot().expect("snapshot");
    let profile = &snapshot["profiles"][0];
    assert_eq!(profile["sourceServerId"], SOURCE_B);
    assert_eq!(profile["lastConnectedAt"], 100);
    assert_eq!(profile["lifecycleState"], "active");
    let server = snapshot["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["sourceServerId"] == SOURCE_B)
        .expect("new source");
    assert_eq!(server["displayName"], "prod-box");
    assert_eq!(server["osUser"], "root");
    assert_eq!(server["hostname"], "box-1");
    assert_eq!(server["lastSeenVersion"], "3.16.2");
    assert_eq!(server["installState"], "installed");
    // 修复原因：握手不得提前写 lastOnlineAt，否则首连 Provisioning 屏障会把
    // 新 Source 误判为「曾在线」，失败首连被降级为 warning。
    assert!(
        server.get("lastOnlineAt").is_none(),
        "首次握手未通过 online 屏障前不应记录 lastOnlineAt: {server}"
    );
    assert_eq!(
        server["declaredWorkspaces"][0]["workspaceIdentity"],
        format!("remote:server:{SOURCE_B}:/srv/work")
    );
    let history = &snapshot["workspaceHistory"][0];
    assert_eq!(history["sourceServerId"], SOURCE_B);
    assert_eq!(
        history["workspaceIdentity"],
        format!("remote:server:{SOURCE_B}:/srv/work")
    );
    assert_eq!(
        history["legacyWorkspaceIdentity"],
        "remote:ssh:a.example:22:dev:/srv/work"
    );
    let persisted = settings::read(&home).await.expect("settings");
    assert_eq!(persisted["unrelated"]["keep"], true);
}

#[tokio::test]
async fn identity_change_without_confirmation_is_zero_write() {
    let (dir, registry, home) = registry_with(fixtures(Some(SOURCE_A))).await;
    let service = credentials(&dir);
    service.save(PASSWORD_KEY, "secret").await.unwrap();
    let path = settings::settings_path(&home);
    let before = std::fs::read(&path).expect("settings bytes");
    let generation = registry.snapshot().expect("snapshot")["generation"].clone();
    let outcome = registry
        .record_handshake(PROFILE, &record(SOURCE_B), false, Some(&service))
        .await
        .expect("handshake");
    assert!(outcome.identity_changed);
    assert_eq!(
        outcome.effective_profile.source_server_id.as_deref(),
        Some(SOURCE_A)
    );
    assert_eq!(std::fs::read(&path).expect("settings bytes"), before);
    assert_eq!(
        registry.snapshot().expect("snapshot")["generation"],
        generation
    );
}

#[tokio::test]
async fn handshake_preserves_existing_last_online_at() {
    let mut value = fixtures(Some(SOURCE_A));
    value["sourceServers"][0]["lastOnlineAt"] = json!(50);
    let (dir, registry, _home) = registry_with(value).await;
    let service = credentials(&dir);
    service.save(PASSWORD_KEY, "secret").await.unwrap();

    let outcome = registry
        .record_handshake(PROFILE, &record(SOURCE_A), false, Some(&service))
        .await
        .expect("handshake");
    assert!(!outcome.identity_changed);
    let snapshot = registry.snapshot().expect("snapshot");
    let server = snapshot["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["sourceServerId"] == SOURCE_A)
        .expect("source");
    assert_eq!(
        server["lastOnlineAt"], 50,
        "握手不得用握手时间覆盖既有 lastOnlineAt"
    );
}

#[tokio::test]
async fn confirmation_retires_old_profile_and_rekeys_credentials() {
    let (dir, registry, home) = registry_with(fixtures(Some(SOURCE_A))).await;
    let service = credentials(&dir);
    service.save(PASSWORD_KEY, "secret").await.unwrap();
    let outcome = registry
        .record_handshake(PROFILE, &record(SOURCE_B), true, Some(&service))
        .await
        .expect("handshake");
    assert!(outcome.identity_changed);
    let new_profile_id = outcome.effective_profile.profile_id.clone();
    assert_ne!(new_profile_id, PROFILE);
    assert_eq!(
        outcome.effective_profile.source_server_id.as_deref(),
        Some(SOURCE_B)
    );
    let snapshot = registry.snapshot().expect("snapshot");
    let profiles = snapshot["profiles"].as_array().unwrap();
    let old = profiles
        .iter()
        .find(|profile| profile["profileId"] == PROFILE)
        .unwrap();
    assert_eq!(old["lifecycleState"], "retired");
    assert_eq!(old["retiredReason"], "source-reinstalled");
    assert_eq!(old["retiredAt"], 100);
    let new = profiles
        .iter()
        .find(|profile| profile["profileId"] == new_profile_id)
        .expect("new profile");
    assert_eq!(new["lifecycleState"], "active");
    let new_key = format!("connection-profile:{new_profile_id}:password");
    assert_eq!(new["target"]["passwordCredentialKey"], new_key);
    assert_eq!(
        service.load(&new_key).await.unwrap().as_deref(),
        Some("secret")
    );
    let servers = snapshot["servers"].as_array().unwrap();
    let old_source = servers
        .iter()
        .find(|server| server["sourceServerId"] == SOURCE_A)
        .unwrap();
    assert_eq!(old_source["installState"], "uninstalled");
    assert!(old_source.get("preferredProfileId").is_none());
    let new_source = servers
        .iter()
        .find(|server| server["sourceServerId"] == SOURCE_B)
        .unwrap();
    assert_eq!(new_source["preferredProfileId"], new_profile_id);
    assert_eq!(snapshot["workspaceHistory"][0]["sourceServerId"], SOURCE_A);
    let persisted = settings::read(&home).await.expect("settings");
    assert_eq!(
        persisted["sourceServerWorkspaceHistory"][0]["sourceServerId"],
        SOURCE_A
    );
}

#[tokio::test]
async fn confirmation_without_credentials_fails_without_writes() {
    let (_dir, registry, home) = registry_with(fixtures(Some(SOURCE_A))).await;
    let path = settings::settings_path(&home);
    let before = std::fs::read(&path).expect("settings bytes");
    let error = registry
        .record_handshake(PROFILE, &record(SOURCE_B), true, None)
        .await
        .expect_err("missing credentials");
    assert!(format!("{error:#}").contains("dataBaseDir"), "{error:#}");
    assert_eq!(std::fs::read(&path).expect("settings bytes"), before);
}

#[test]
fn declared_workspaces_use_canonical_identity_and_dedupe() {
    let workspaces = vec![
        DeclaredWorkspaceInput {
            path: " /srv//work/ ".to_owned(),
            workspace_identity: Some("remote:ssh:host:22:dev:/srv/work".to_owned()),
            label: None,
        },
        DeclaredWorkspaceInput {
            path: "/srv/work".to_owned(),
            workspace_identity: None,
            label: Some("label".to_owned()),
        },
        DeclaredWorkspaceInput {
            path: "/".to_owned(),
            workspace_identity: None,
            label: None,
        },
    ];
    let result = project_declared_workspaces(SOURCE_A, &workspaces);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].workspace_path, "/srv//work/");
    assert_eq!(
        result[0].workspace_identity,
        format!("remote:server:{SOURCE_A}:/srv/work")
    );
    assert_eq!(result[0].label.as_deref(), Some("work"));
    assert_eq!(result[1].workspace_path, "/");
    assert_eq!(
        result[1].workspace_identity,
        format!("remote:server:{SOURCE_A}:/")
    );
    assert_eq!(result[1].label.as_deref(), Some("/"));
}
