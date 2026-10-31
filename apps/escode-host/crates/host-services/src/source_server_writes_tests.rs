use serde_json::{Value, json};

use super::*;
use crate::credential::CredentialService;
use crate::settings;

const SOURCE_A: &str = "11111111-1111-4111-8111-111111111111";
const SOURCE_B: &str = "22222222-2222-4222-8222-222222222222";
const PROFILE_A: &str = "33333333-3333-4333-8333-333333333333";
const PROFILE_A2: &str = "44444444-4444-4444-8444-444444444444";
const PROFILE_B: &str = "55555555-5555-4555-8555-555555555555";

fn fixtures() -> Value {
    json!({
        "sourceServers": [
            {"sourceServerId": SOURCE_A, "displayName": "box", "preferredProfileId": PROFILE_A},
            {"sourceServerId": SOURCE_B, "displayName": "other"},
        ],
        "connectionProfiles": [
            {
                "profileId": PROFILE_A,
                "sourceServerId": SOURCE_A,
                "transport": "ssh",
                "target": {
                    "kind": "ssh",
                    "host": "a.example",
                    "username": "dev",
                    "passwordCredentialKey": "connection-profile:PROFILE_A:password",
                },
                "createdAt": 1,
                "displayName": "primary",
            },
            {
                "profileId": PROFILE_A2,
                "sourceServerId": SOURCE_A,
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "b.example", "username": "dev"},
                "createdAt": 2,
            },
            {
                "profileId": PROFILE_B,
                "sourceServerId": SOURCE_B,
                "transport": "docker",
                "target": {"kind": "docker", "container": "box"},
                "createdAt": 3,
            },
        ],
        "sourceServerWorkspaceHistory": [
            {"workspacePath": "/a", "profileId": PROFILE_A, "sourceServerId": SOURCE_A, "lastOpenedAt": 5},
            {"workspacePath": "/b", "profileId": PROFILE_B, "sourceServerId": SOURCE_B, "lastOpenedAt": 6},
        ],
        "remoteConversationWorkspaceBySourceServerId": {
            SOURCE_A: {"workspacePath": "/a", "lastResolvedAt": 7},
        },
        "unrelated": {"keep": true},
    })
}

async fn registry_with(
    settings_value: Value,
) -> (tempfile::TempDir, SourceServerRegistry, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("dir");
    let home = dir.path().join("home");
    let path = settings::settings_path(&home);
    std::fs::create_dir_all(path.parent().unwrap()).expect("settings dir");
    std::fs::write(&path, serde_json::to_vec_pretty(&settings_value).unwrap()).expect("settings");
    let registry = SourceServerRegistry::load(&home);
    (dir, registry, home)
}

fn credentials(dir: &tempfile::TempDir) -> CredentialService {
    CredentialService::new(CredentialService::path(dir.path()), "test-secret")
}

async fn persisted(home: &std::path::Path) -> Value {
    settings::read(home).await.expect("settings")
}

#[tokio::test]
async fn rename_source_and_profile_persist_and_bump_generation_once() {
    let (dir, registry, home) = registry_with(fixtures()).await;
    let before = registry.snapshot().unwrap()["generation"].as_u64().unwrap();

    registry
        .rename_source(SOURCE_A, "  renamed  ")
        .await
        .unwrap();
    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot["servers"][0]["displayName"], json!("renamed"));
    assert_eq!(snapshot["generation"], json!(before + 1));

    registry.rename_profile(PROFILE_A2, "second").await.unwrap();
    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot["profiles"][1]["displayName"], json!("second"));
    assert_eq!(snapshot["generation"], json!(before + 2));

    let stored = persisted(&home).await;
    assert_eq!(stored["unrelated"], json!({"keep": true}));
    assert_eq!(stored["sourceServers"][0]["displayName"], json!("renamed"));
    drop(dir);
}

#[tokio::test]
async fn rename_rejects_empty_and_unknown_records_without_writing() {
    let (_dir, registry, home) = registry_with(fixtures()).await;
    assert!(registry.rename_source(SOURCE_A, "   ").await.is_err());
    assert!(registry.rename_profile("missing", "name").await.is_err());
    assert!(registry.rename_source("missing", "name").await.is_err());
    assert_eq!(registry.snapshot().unwrap()["generation"], json!(1));
    let stored = persisted(&home).await;
    let expected = fixtures();
    for key in [
        "sourceServers",
        "connectionProfiles",
        "sourceServerWorkspaceHistory",
        "remoteConversationWorkspaceBySourceServerId",
        "unrelated",
    ] {
        assert_eq!(stored[key], expected[key], "{key} must stay unchanged");
    }
}

#[tokio::test]
async fn set_preferred_profile_requires_same_active_source() {
    let (_dir, registry, _home) = registry_with(fixtures()).await;
    let error = registry
        .set_preferred_profile(SOURCE_A, PROFILE_B)
        .await
        .expect_err("cross-source profile must be rejected");
    assert!(format!("{error:#}").contains("does not belong"));

    registry
        .set_preferred_profile(SOURCE_A, PROFILE_A2)
        .await
        .unwrap();
    assert_eq!(
        registry.snapshot().unwrap()["servers"][0]["preferredProfileId"],
        json!(PROFILE_A2)
    );
}

#[tokio::test]
async fn remove_profile_reassigns_preferred_and_deletes_credentials() {
    let (dir, registry, home) = registry_with(fixtures()).await;
    let service = credentials(&dir);
    let password_key = "connection-profile:PROFILE_A:password";
    service.save(password_key, "secret").await.unwrap();

    registry
        .remove_profile(PROFILE_A, Some(&service))
        .await
        .unwrap();

    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot["profiles"].as_array().unwrap().len(), 2);
    assert!(
        snapshot["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|profile| profile["profileId"] != json!(PROFILE_A))
    );
    assert_eq!(
        snapshot["servers"][0]["preferredProfileId"],
        json!(PROFILE_A2)
    );
    assert_eq!(snapshot["workspaceHistory"].as_array().unwrap().len(), 1);
    assert_eq!(service.load(password_key).await.unwrap(), None);

    let stored = persisted(&home).await;
    assert_eq!(stored["unrelated"], json!({"keep": true}));
    assert_eq!(stored["connectionProfiles"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn remove_missing_profile_is_a_noop() {
    let (_dir, registry, _home) = registry_with(fixtures()).await;
    registry.remove_profile("missing", None).await.unwrap();
    assert_eq!(registry.snapshot().unwrap()["generation"], json!(1));
}

#[tokio::test]
async fn removing_ssh_profile_without_a_credentials_store_fails_before_writing() {
    let (_dir, registry, _home) = registry_with(fixtures()).await;
    let error = registry
        .remove_profile(PROFILE_A, None)
        .await
        .expect_err("SSH credentials must not be silently left behind");
    assert!(format!("{error:#}").contains("dataBaseDir"));
    assert_eq!(
        registry.snapshot().unwrap()["profiles"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn remove_source_server_clears_history_and_conversation_map_once() {
    let (dir, registry, home) = registry_with(fixtures()).await;
    registry
        .remove_source_server(SOURCE_A, Some(&credentials(&dir)))
        .await
        .unwrap();

    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot["servers"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["profiles"][0]["profileId"], json!(PROFILE_B));
    assert_eq!(snapshot["workspaceHistory"].as_array().unwrap().len(), 1);

    let stored = persisted(&home).await;
    assert_eq!(
        stored["remoteConversationWorkspaceBySourceServerId"],
        json!({})
    );
    assert_eq!(stored["unrelated"], json!({"keep": true}));

    let generation = registry.snapshot().unwrap()["generation"].as_u64().unwrap();
    registry.remove_source_server(SOURCE_A, None).await.unwrap();
    assert_eq!(
        registry.snapshot().unwrap()["generation"],
        json!(generation),
        "absent source must not bump generation"
    );
}

#[tokio::test]
async fn writes_require_a_settings_home() {
    let registry = SourceServerRegistry::from_settings(&fixtures()).unwrap();
    let error = registry
        .rename_source(SOURCE_A, "name")
        .await
        .expect_err("in-memory registry cannot persist");
    assert!(format!("{error:#}").contains("settings home"));
}
