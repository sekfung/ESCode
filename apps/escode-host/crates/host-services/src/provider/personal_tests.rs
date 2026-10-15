use serde_json::json;

use super::*;

fn doc() -> Doc {
    Doc::decode(&json!({
        "schemaVersion": 1,
        "config": {
            "providerConfigRules": {"providerRules": [
                {"providerId": "p1", "providerName": "P", "config": {"group": "standard-personal", "personalModelIds": ["m1"]}}
            ]},
            "modelConfigRules": {
                "providerModelRules": [{"providerId": "p1", "modelId": "m1", "config": {"enabled": true}}],
                "manualProviderModelRules": []
            }
        }
    }))
    .expect("decode")
}

#[test]
fn encode_round_trips_and_distinguishes_missing_order_from_empty() {
    let mut value = doc();
    assert!(value.provider_order.is_none());
    assert!(value.encode()["config"].get("providerOrder").is_none());
    let before = value.revision();
    value.provider_order = Some(Vec::new());
    assert_ne!(before, value.revision(), "空数组与缺省是不同内容");
    let decoded = Doc::decode(&value.encode()).expect("round trip");
    assert_eq!(decoded.provider_order, Some(Vec::new()));
}

#[test]
fn set_exact_replaces_in_place_and_validates_by_mode() {
    let mut value = doc();
    value
        .set_exact("p1", "m1", json!({"enabled": false}), None)
        .expect("smart");
    assert_eq!(value.exact.len(), 1);
    assert_eq!(
        value.exact_config("p1", "m1"),
        Some(json!({"enabled": false}))
    );
    assert!(
        value
            .set_exact("p1", "m2", json!({"enabled": true}), Some(false))
            .is_err(),
        "手动规则必须齐全"
    );
    assert!(value.exact_rule("p1", "m2").is_none(), "校验失败不留半成品");
}

#[test]
fn canonical_rejects_account_provider_access() {
    let mut value = doc();
    value.providers.0.push(ProviderRule {
        provider_id: "account:x".into(),
        config: json!({"access": {"type": "api-key"}}),
        ..ProviderRule::default()
    });
    assert!(value.canonical().is_err());
}
