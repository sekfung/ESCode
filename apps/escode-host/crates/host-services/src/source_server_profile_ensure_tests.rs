use serde_json::{Value, json};

use super::*;
use crate::{credential::CredentialService, settings};

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

fn ssh_target(password: Option<&str>) -> Value {
    let mut target = json!({
        "kind": "ssh",
        "host": "Example.Test",
        "port": 2222,
        "username": "dev",
        "sshConfigAlias": "build-box",
        "assetInstallMode": "remote-download",
    });
    if let Some(password) = password {
        target["password"] = json!(password);
    }
    target
}

#[tokio::test]
async fn creates_profile_without_persisting_plaintext_credentials() {
    let (dir, registry, home) = registry_with(json!({})).await;
    let service = credentials(&dir);

    let profile = registry
        .ensure_profile(&ssh_target(Some("s3cret")), Some(&service))
        .await
        .unwrap();

    assert_eq!(profile.transport, "ssh");
    assert_eq!(profile.display_name.as_deref(), Some("build-box"));
    assert_eq!(profile.source_server_id, None);
    assert_eq!(profile.target["host"], json!("example.test"));
    assert_eq!(profile.target["port"], json!(2222));
    assert_eq!(
        profile.target["passwordCredentialKey"],
        json!(format!(
            "connection-profile:{}:password",
            profile.profile_id
        ))
    );
    assert!(profile.target.get("password").is_none());
    let key = profile.target["passwordCredentialKey"].as_str().unwrap();
    assert_eq!(service.load(key).await.unwrap().as_deref(), Some("s3cret"));

    let stored = settings::read(&home).await.unwrap();
    assert_eq!(stored["connectionProfiles"].as_array().unwrap().len(), 1);
    assert!(!stored.to_string().contains("s3cret"));
}

#[tokio::test]
async fn matches_existing_profile_by_connection_identity_not_display_fields() {
    let (dir, registry, _home) = registry_with(json!({})).await;
    let service = credentials(&dir);
    let first = registry
        .ensure_profile(&ssh_target(Some("s3cret")), Some(&service))
        .await
        .unwrap();

    let mut second_target = ssh_target(None);
    second_target["sshConfigAlias"] = json!("another-alias");
    second_target["assetInstallMode"] = json!("local-download-upload");
    second_target["resourcePackages"] = json!({"selectedPackageIds": ["ripgrep"]});
    let second = registry
        .ensure_profile(&second_target, Some(&service))
        .await
        .unwrap();

    assert_eq!(second.profile_id, first.profile_id);
    assert_eq!(
        second.target["passwordCredentialKey"], first.target["passwordCredentialKey"],
        "无新明文时必须保留已保存凭据引用"
    );
    assert_eq!(
        registry.snapshot().unwrap()["profiles"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn concurrent_first_connect_creates_one_profile() {
    let (dir, registry, _home) = registry_with(json!({})).await;
    let service = credentials(&dir);
    let target = ssh_target(None);

    let (left, right) = tokio::join!(
        registry.ensure_profile(&target, Some(&service)),
        registry.ensure_profile(&target, Some(&service))
    );
    assert_eq!(left.unwrap().profile_id, right.unwrap().profile_id);
    assert_eq!(
        registry.snapshot().unwrap()["profiles"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn existing_profile_keeps_source_binding_when_target_is_ensured_again() {
    let (_dir, registry, _home) = registry_with(json!({
        "connectionProfiles": [{
            "profileId": "33333333-3333-4333-8333-333333333333",
            "sourceServerId": "11111111-1111-4111-8111-111111111111",
            "transport": "ssh",
            "target": {
                "kind": "ssh",
                "host": "example.test",
                "port": 2222,
                "username": "dev",
                "privateKeyPath": "/keys/dev",
                "privateKeyPassphraseCredentialKey": "connection-profile:33333333-3333-4333-8333-333333333333:private-key-passphrase"
            },
            "createdAt": 1
        }]
    }))
    .await;

    let profile = registry
        .ensure_profile(
            &json!({
                "kind": "ssh",
                "host": "example.test",
                "port": 2222,
                "username": "dev",
                "privateKeyPath": "/keys/dev"
            }),
            None,
        )
        .await
        .unwrap();

    assert_eq!(profile.profile_id, "33333333-3333-4333-8333-333333333333");
    assert_eq!(
        profile.source_server_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(
        profile.target["privateKeyPassphraseCredentialKey"],
        json!("connection-profile:33333333-3333-4333-8333-333333333333:private-key-passphrase")
    );
}

#[tokio::test]
async fn invalid_target_is_rejected_without_writing() {
    let (_dir, registry, home) = registry_with(json!({ "unrelated": true })).await;
    let error = registry
        .ensure_profile(&json!({"kind": "ssh", "host": "", "username": ""}), None)
        .await
        .expect_err("invalid target");
    assert!(format!("{error:#}").contains("invalid"));
    let stored = settings::read(&home).await.unwrap();
    assert_eq!(stored["unrelated"], json!(true));
    assert_eq!(stored["connectionProfiles"], json!([]));
}

#[tokio::test]
async fn creates_server_profile_with_token_only_in_credentials() {
    let (dir, registry, home) = registry_with(json!({})).await;
    let service = credentials(&dir);

    let profile = registry
        .ensure_profile(
            &json!({
                "kind": "server",
                "url": "  https://studio.example.com/escode/  ",
                "token": "s3cret-token",
                "workspacePath": "/srv/work",
            }),
            Some(&service),
        )
        .await
        .unwrap();

    assert_eq!(profile.transport, "server");
    assert_eq!(profile.target["kind"], json!("server"));
    assert_eq!(
        profile.target["url"],
        json!("https://studio.example.com/escode/")
    );
    assert_eq!(
        profile.target["tokenCredentialKey"],
        json!(format!("connection-profile:{}:token", profile.profile_id))
    );
    assert!(profile.target.get("token").is_none());
    assert!(
        profile.target.get("workspacePath").is_none(),
        "workspacePath 属于会话范围，不进入 Profile 身份"
    );
    let key = profile.target["tokenCredentialKey"].as_str().unwrap();
    assert_eq!(
        service.load(key).await.unwrap().as_deref(),
        Some("s3cret-token")
    );
    let stored = settings::read(&home).await.unwrap();
    assert!(!stored.to_string().contains("s3cret-token"));
}

#[tokio::test]
async fn server_profile_identity_uses_trimmed_url_only() {
    let (dir, registry, _home) = registry_with(json!({})).await;
    let service = credentials(&dir);
    let first = registry
        .ensure_profile(
            &json!({"kind": "server", "url": "https://studio.example.com"}),
            Some(&service),
        )
        .await
        .unwrap();
    let second = registry
        .ensure_profile(
            &json!({"kind": "server", "url": "  https://studio.example.com  "}),
            None,
        )
        .await
        .unwrap();
    let websocket = registry
        .ensure_profile(
            &json!({"kind": "server", "url": "wss://studio.example.com"}),
            None,
        )
        .await
        .unwrap();

    assert_eq!(second.profile_id, first.profile_id);
    assert_ne!(
        websocket.profile_id, first.profile_id,
        "协议不同是不同目标，不做 http/ws 互转"
    );
}

#[tokio::test]
async fn server_profile_keeps_token_reference_when_reconnect_has_no_token() {
    let (dir, registry, _home) = registry_with(json!({})).await;
    let service = credentials(&dir);
    let first = registry
        .ensure_profile(
            &json!({"kind": "server", "url": "https://studio.example.com", "token": "abc"}),
            Some(&service),
        )
        .await
        .unwrap();

    let second = registry
        .ensure_profile(
            &json!({"kind": "server", "url": "https://studio.example.com"}),
            Some(&service),
        )
        .await
        .unwrap();

    assert_eq!(second.profile_id, first.profile_id);
    assert_eq!(
        second.target["tokenCredentialKey"], first.target["tokenCredentialKey"],
        "未携带新 token 时必须保留已有引用"
    );
}
