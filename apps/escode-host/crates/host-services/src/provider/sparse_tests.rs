use serde_json::json;

use super::*;

#[test]
fn provider_config_rejects_unknown_fields_and_bad_enums() {
    assert!(provider_config(&json!({})).is_ok());
    assert!(
        provider_config(
            &json!({"api":{"type":"openai-responses","baseUrl":"https://a.example/v1"}})
        )
        .is_ok()
    );
    assert!(provider_config(&json!({"extra":1})).is_err());
    assert!(provider_config(&json!({"api":{"type":"nope"}})).is_err());
    assert!(provider_config(&json!({"api":{"baseUrl":"not a url"}})).is_err());
    assert!(provider_config(&json!({"personalModelIds":[""]})).is_err());
    assert!(provider_config(&json!({"access":{"type":"api-key","apiKey":null}})).is_ok());
    assert!(provider_config(&json!({"access":{"type":"zhipu-account","mode":"bad"}})).is_err());
}

#[test]
fn model_config_is_sparse_but_typed() {
    assert!(model_config(&json!({})).is_ok());
    assert!(model_config(&json!({"enabled":null,"properties":{"contextWindow":8000}})).is_ok());
    assert!(model_config(&json!({"properties":{"contextWindow":0}})).is_err());
    assert!(model_config(&json!({"optionSpecs":{"reasoningLevel":{"values":["a","a"]}}})).is_err());
    assert!(model_config(&json!({"optionSpecs":{"reasoningLevel":{"values":[]}}})).is_err());
}

#[test]
fn manual_config_requires_every_editable_leaf() {
    let full = json!({
        "enabled": true,
        "properties": {
            "contextWindow": 1000, "supportsJsonSchemaOutput": true,
            "supportsNativeWebSearch": false, "supportsMidConversationSystem": true,
            "inputFormat": {"supportsImage": true, "supportsVideo": false, "supportsPdf": false}
        },
        "optionSpecs": {
            "reasoningLevel": {"values": ["low","high"], "map": "x"},
            "maxOutputTokens": {"max": 4096}
        }
    });
    assert!(manual_model_config(&full).is_ok());
    assert!(manual_model_config(&json!({"enabled": true})).is_err());
    let mut missing = full;
    missing["properties"]["inputFormat"]
        .as_object_mut()
        .unwrap()
        .remove("supportsPdf");
    assert!(manual_model_config(&missing).is_err());
}
