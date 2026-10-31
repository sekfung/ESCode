//! S4-g Provider Provisioning Envelope 来源：Personal/设置/凭据 allowlist。

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::credential::CredentialService;
use crate::provider::ProviderService;

fn builtin() -> Value {
    json!({
        "schemaVersion": 1, "revision": 1,
        "config": {
            "providerConfigRules": {"templateRules": [], "providerRules": [{
                "providerId": "b",
                "config": {"group": "zai-family", "access": {"type": "api-key", "apiKey": "k"},
                           "api": {"type": "openai-responses", "baseUrl": "https://b.example.com"},
                           "builtinModelIds": ["m"]}}]},
            "modelConfigRules": {"modelRules": [{"modelMatch": ".*", "config": {
                "enabled": true,
                "properties": {"requiresMfjsToolSchema": false, "contextWindow": 10,
                    "inputFormat": {"supportsText": true, "supportsImage": false, "supportsVideo": false, "supportsAudio": false, "supportsPdf": false},
                    "outputFormat": {"supportsText": true}, "supportsToolCall": true, "supportsJsonSchemaOutput": false,
                    "supportsNativeWebSearch": false, "supportsMidConversationSystem": false},
                "optionSpecs": {"reasoningLevel": {"values": ["low"], "map": "{}"}, "maxOutputTokens": {"max": 5, "map": "{}"}}}}],
                "modelApiRules": [], "providerSiteRules": [], "templateModelRules": [], "builtinProviderModelRules": []}
        }
    })
}

fn personal() -> Value {
    json!({"schemaVersion": 1, "config": {
        "providerOrder": ["p1"],
        "providerConfigRules": {"providerRules": [{
            "providerId": "p1",
            "providerName": "Personal One",
            "enabled": true,
            "config": {
                "group": "standard-personal",
                "access": {"type": "api-key", "apiKey": "local-key"},
                "api": {"type": "openai-responses", "baseUrl": "https://local.example.com"},
                "personalModelIds": ["m1"]
            }
        }]},
        "modelConfigRules": {"providerModelRules": [], "manualProviderModelRules": []},
        "defaultModelSelection": {"providerId": "p1", "modelId": "m1"}
    }})
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    provider: Arc<ProviderService>,
    credentials: Arc<CredentialService>,
}

async fn fixture(with_personal: bool) -> Fixture {
    let dir = tempfile::tempdir().expect("dir");
    let home = dir.path().to_path_buf();
    let builtin_path = home.join("builtin.json");
    let personal_path = home.join(".escode").join("v2").join("provider_config.json");
    tokio::fs::write(&builtin_path, builtin().to_string())
        .await
        .expect("builtin");
    if with_personal {
        tokio::fs::create_dir_all(personal_path.parent().unwrap())
            .await
            .expect("dirs");
        tokio::fs::write(&personal_path, personal().to_string())
            .await
            .expect("personal");
    }
    let provider = Arc::new(ProviderService::new(builtin_path, personal_path));
    let credentials = Arc::new(CredentialService::new(
        CredentialService::path(&home),
        "secret",
    ));
    Fixture {
        _dir: dir,
        home,
        provider,
        credentials,
    }
}

async fn write_settings(home: &std::path::Path, value: Value) {
    let path = crate::settings::settings_path(home);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .expect("dirs");
    tokio::fs::write(path, value.to_string())
        .await
        .expect("settings");
}

fn source(fixture: &Fixture) -> LocalProviderProvisioningSource {
    LocalProviderProvisioningSource::new(
        Arc::clone(&fixture.provider),
        fixture.home.clone(),
        Arc::clone(&fixture.credentials),
    )
}

#[tokio::test]
async fn envelope_contains_personal_settings_and_allowlisted_credentials() {
    let fixture = fixture(true).await;
    write_settings(
        &fixture.home,
        json!({
            "providerFamilyDomain": "zai",
            "providerFamilyConnectionSelections": {
                "zai": {"kind": "team-coding-plan", "productId": "p", "organizationId": "o", "projectId": "j"}
            }
        }),
    )
    .await;
    fixture
        .credentials
        .save("oauth:active_provider", "zai")
        .await
        .expect("credential");
    fixture
        .credentials
        .save("oauth:zai:access_token", "token-value")
        .await
        .expect("credential");
    fixture
        .credentials
        .save("unrelated:key", "ignored")
        .await
        .expect("credential");

    let envelope = source(&fixture).read("sync-1").await.expect("envelope");

    assert_eq!(envelope["schemaVersion"], 2);
    assert_eq!(envelope["syncId"], "sync-1");
    let providers = envelope["personalConfig"]["providerConfigRules"]["providerRules"]
        .as_array()
        .expect("providers");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["providerId"], "p1");
    assert_eq!(envelope["personalConfig"]["providerOrder"], json!(["p1"]));
    assert_eq!(envelope["accountSettings"]["providerFamilyDomain"], "zai");
    assert_eq!(
        envelope["accountSettings"]["providerFamilyConnectionSelections"]["zai"]["kind"],
        "team-coding-plan"
    );
    let credentials = envelope["credentials"].as_array().expect("credentials");
    assert_eq!(credentials.len(), 2);
    assert!(
        credentials
            .iter()
            .all(|entry| entry["scope"] == "oauth-session")
    );
    assert!(
        credentials
            .iter()
            .any(|entry| entry["key"] == "oauth:active_provider" && entry["value"] == "zai")
    );
    assert!(
        credentials
            .iter()
            .all(|entry| entry["key"] != "unrelated:key")
    );
}

#[tokio::test]
async fn missing_personal_file_is_allowed_only_for_empty_config() {
    let empty_fixture = fixture(false).await;
    let envelope = source(&empty_fixture)
        .read("sync-empty")
        .await
        .expect("empty envelope");
    assert_eq!(
        envelope["personalConfig"]["providerConfigRules"]["providerRules"],
        json!([])
    );
    assert_eq!(envelope["personalConfig"]["providerOrder"], json!([]));
}

#[tokio::test]
async fn broken_personal_file_fails_instead_of_exporting_empty_config() {
    let fixture = fixture(true).await;
    let path = fixture
        .home
        .join(".escode")
        .join("v2")
        .join("provider_config.json");
    tokio::fs::write(&path, "{not json").await.expect("write");
    let error = source(&fixture)
        .read("sync-broken")
        .await
        .expect_err("must fail");
    assert!(format!("{error:#}").contains("Personal Provider Config"));
}

#[tokio::test]
async fn non_allowlisted_credential_junk_is_ignored_but_allowlisted_junk_fails() {
    let fixture = fixture(false).await;
    let path = CredentialService::path(&fixture.home);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .expect("dirs");
    tokio::fs::write(
        &path,
        json!({"unrelated": 123, "oauth:active_provider": "plain-value"}).to_string(),
    )
    .await
    .expect("credentials");
    let envelope = source(&fixture).read("sync-junk").await.expect("envelope");
    let credentials = envelope["credentials"].as_array().expect("credentials");
    assert_eq!(credentials.len(), 1);
    assert_eq!(credentials[0]["value"], "plain-value");

    tokio::fs::write(&path, json!({"oauth:active_provider": 123}).to_string())
        .await
        .expect("credentials");
    let error = source(&fixture)
        .read("sync-bad-cred")
        .await
        .expect_err("must fail");
    assert!(format!("{error:#}").contains("must be a string"));
}

#[tokio::test]
async fn empty_allowlisted_credential_is_skipped() {
    let fixture = fixture(false).await;
    // 明文空值（以及未来可解密的空密文）解密后 trim 为空；此时不进入 Envelope。
    let path = CredentialService::path(&fixture.home);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .expect("dirs");
    tokio::fs::write(&path, json!({"oauth:active_provider": ""}).to_string())
        .await
        .expect("credentials");
    let envelope = source(&fixture)
        .read("sync-empty-cred")
        .await
        .expect("envelope");
    assert_eq!(envelope["credentials"], json!([]));
}

#[tokio::test]
async fn invalid_account_settings_fail_explicitly() {
    let fixture = fixture(false).await;
    write_settings(&fixture.home, json!({"providerFamilyDomain": "unknown"})).await;
    let error = source(&fixture)
        .read("sync-settings")
        .await
        .expect_err("must fail");
    assert!(format!("{error:#}").contains("providerFamilyDomain"));
}
