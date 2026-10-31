use serde_json::json;

use super::*;
use crate::credential::CredentialService;
use crate::settings;
use crate::source_server::SourceServerRegistry;

const SOURCE: &str = "11111111-1111-4111-8111-111111111111";
const PROFILE: &str = "22222222-2222-4222-8222-222222222222";
const PASSWORD_KEY: &str = "connection-profile:PROFILE:password";

fn fixtures(target: serde_json::Value) -> serde_json::Value {
    json!({
        "sourceServers": [{"sourceServerId": SOURCE, "displayName": "box"}],
        "connectionProfiles": [{
            "profileId": PROFILE,
            "sourceServerId": SOURCE,
            "transport": "ssh",
            "target": target,
            "createdAt": 1,
            "lastConnectError": "stale",
        }],
        "sourceServerWorkspaceHistory": [],
        "unrelated": {"keep": true},
    })
}

async fn registry_with(
    settings_value: serde_json::Value,
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

#[tokio::test]
async fn password_to_private_key_rotates_credentials_and_normalizes_target() {
    let (dir, registry, home) = registry_with(fixtures(json!({
        "kind": "ssh",
        "host": "a.example",
        "username": "dev",
        "passwordCredentialKey": PASSWORD_KEY,
    })))
    .await;
    let service = credentials(&dir);
    service.save(PASSWORD_KEY, "secret").await.unwrap();

    let input = UpdateProfileInput {
        profile_id: PROFILE.to_owned(),
        profile_display_name: "  renamed  ".to_owned(),
        source_display_name: Some("  new box ".to_owned()),
        target: json!({
            "kind": "ssh",
            "host": "B.Example",
            "port": 2222,
            "username": " dev ",
            "privateKeyPath": "/key",
        }),
        ssh_auth_method: Some("privateKey".to_owned()),
    };
    registry
        .update_profile(input, Some(&service))
        .await
        .unwrap();

    let snapshot = registry.snapshot().unwrap();
    assert_eq!(snapshot["profiles"][0]["displayName"], json!("renamed"));
    assert_eq!(
        snapshot["profiles"][0]["target"]["host"],
        json!("b.example")
    );
    assert_eq!(snapshot["profiles"][0]["target"]["port"], json!(2222));
    assert!(
        snapshot["profiles"][0]["target"]
            .get("passwordCredentialKey")
            .is_none()
    );
    assert!(snapshot["profiles"][0].get("lastConnectError").is_none());
    assert_eq!(snapshot["servers"][0]["displayName"], json!("new box"));
    assert_eq!(service.load(PASSWORD_KEY).await.unwrap(), None);

    let stored = settings::read(&home).await.unwrap();
    assert_eq!(stored["unrelated"], json!({"keep": true}));
    assert!(!stored.to_string().contains("secret"));
}

#[tokio::test]
async fn password_auth_requires_entered_or_saved_password() {
    let (_dir, registry, home) = registry_with(fixtures(json!({
        "kind": "ssh",
        "host": "a.example",
        "username": "dev",
    })))
    .await;
    let input = UpdateProfileInput {
        profile_id: PROFILE.to_owned(),
        profile_display_name: "renamed".to_owned(),
        source_display_name: None,
        target: json!({"kind": "ssh", "host": "a.example", "username": "dev"}),
        ssh_auth_method: Some("password".to_owned()),
    };
    let error = registry
        .update_profile(input, Some(&credentials(&_dir)))
        .await
        .expect_err("missing password must be rejected");
    assert!(format!("{error:#}").contains("SSH password is required"));
    let stored = settings::read(&home).await.unwrap();
    assert_eq!(
        stored["connectionProfiles"][0]["lastConnectError"],
        json!("stale")
    );
    assert_eq!(
        stored["connectionProfiles"][0]["target"],
        json!({
            "kind": "ssh",
            "host": "a.example",
            "username": "dev",
        })
    );
}

#[tokio::test]
async fn update_rejects_transport_change_and_retired_profiles() {
    let (_dir, registry, _home) = registry_with(fixtures(json!({
        "kind": "ssh",
        "host": "a.example",
        "username": "dev",
    })))
    .await;
    let mut input = UpdateProfileInput {
        profile_id: PROFILE.to_owned(),
        profile_display_name: "renamed".to_owned(),
        source_display_name: None,
        target: json!({"kind": "docker", "container": "box"}),
        ssh_auth_method: None,
    };
    let error = registry
        .update_profile(input.clone(), None)
        .await
        .expect_err("transport cannot change");
    assert!(format!("{error:#}").contains("transport cannot be changed"));

    input.target = json!({"kind": "ssh", "host": "a.example", "username": "dev"});
    let error = registry
        .update_profile(input, None)
        .await
        .expect_err("ssh needs credentials");
    assert!(format!("{error:#}").contains("dataBaseDir"));
}

#[test]
fn normalize_target_enforces_strict_shapes() {
    assert_eq!(
        normalize_target(&json!({"kind": "ssh", "host": " A.Example ", "username": " dev "}))
            .unwrap(),
        json!({"kind": "ssh", "host": "a.example", "port": 22, "username": "dev"})
    );
    assert!(
        normalize_target(&json!({"kind": "ssh", "host": "a", "username": "u", "port": 0})).is_err()
    );
    assert!(normalize_target(&json!({"kind": "wsl", "user": "bad/user"})).is_err());
    assert!(normalize_target(&json!({"kind": "docker", "container": " "})).is_err());
    assert_eq!(
        normalize_target(&json!({"kind": "ssh", "host": "a", "username": "u", "resourcePackages": {"selectedPackageIds": ["unknown"]}}))
            .unwrap(),
        json!({"kind": "ssh", "host": "a", "port": 22, "username": "u"})
    );
}
