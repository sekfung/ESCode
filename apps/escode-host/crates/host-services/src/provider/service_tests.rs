//! `ProviderService` 的缓存与修订号语义（文件指纹闸门 + 内容指纹）。

use serde_json::{Value, json};

use super::*;

pub(super) fn builtin() -> Value {
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

fn personal(order: &[&str]) -> Value {
    json!({"schemaVersion": 1, "config": {"providerOrder": order,
        "providerConfigRules": {"providerRules": []},
        "modelConfigRules": {"providerModelRules": [], "manualProviderModelRules": []}}})
}

async fn revision(service: &ProviderService) -> u64 {
    service.model_selection_view(None).await.expect("view")["revision"]
        .as_u64()
        .expect("revision")
}

#[tokio::test]
async fn revision_advances_only_when_the_content_changes() {
    let dir = tempfile::tempdir().expect("dir");
    let (builtin_path, personal_path) = (dir.path().join("b.json"), dir.path().join("p.json"));
    tokio::fs::write(&builtin_path, builtin().to_string())
        .await
        .expect("write");
    let service = ProviderService::new(builtin_path, personal_path.clone());
    assert_eq!(revision(&service).await, 1, "无 Personal 文件也能加载");
    assert_eq!(revision(&service).await, 1, "指纹未变：复用上次结果");
    tokio::fs::write(&personal_path, personal(&["x"]).to_string())
        .await
        .expect("write");
    assert_eq!(revision(&service).await, 2, "文件出现 → 重新解析");
    // 同内容、不同格式重写：指纹变了要真读，但解析后内容相同 → 修订号不推进。
    tokio::fs::write(
        &personal_path,
        serde_json::to_string_pretty(&personal(&["x"])).expect("json"),
    )
    .await
    .expect("write");
    assert_eq!(revision(&service).await, 2);
    tokio::fs::write(&personal_path, personal(&["y"]).to_string())
        .await
        .expect("write");
    assert_eq!(revision(&service).await, 3);
}

#[tokio::test]
async fn a_broken_builtin_file_fails_loudly_and_recovers_when_fixed() {
    let dir = tempfile::tempdir().expect("dir");
    let (builtin_path, personal_path) = (dir.path().join("b.json"), dir.path().join("p.json"));
    tokio::fs::write(&builtin_path, "{not json")
        .await
        .expect("write");
    let service = ProviderService::new(builtin_path.clone(), personal_path);
    assert!(service.model_selection_view(None).await.is_err());
    tokio::fs::write(&builtin_path, builtin().to_string())
        .await
        .expect("write");
    assert_eq!(revision(&service).await, 1, "失败不进缓存，修好后立刻恢复");
}
