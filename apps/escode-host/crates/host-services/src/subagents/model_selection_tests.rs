//! `modelSelection` 校验（`rust-u2-subagents.md` §2，TS `modelSelectionSchema`）。
use super::*;

#[test]
fn valid_selections_are_trimmed_and_rebuilt() {
    assert_eq!(
        normalize(Some(&serde_json::json!({
            "providerId": " glm ",
            "modelId": " glm-4 ",
            "options": {"reasoningLevel": " high "}
        })))
        .unwrap(),
        Some(serde_json::json!({
            "providerId": "glm",
            "modelId": "glm-4",
            "options": {"reasoningLevel": "high"}
        }))
    );
    // 无 options / options 里 reasoningLevel 为空 → 不带 options 键。
    for value in [
        serde_json::json!({"providerId": "p", "modelId": "m"}),
        serde_json::json!({"providerId": "p", "modelId": "m", "options": {}}),
        serde_json::json!({"providerId": "p", "modelId": "m", "options": {"reasoningLevel": "  "}}),
    ] {
        assert_eq!(
            normalize(Some(&value)).unwrap(),
            Some(serde_json::json!({"providerId": "p", "modelId": "m"})),
            "{value}"
        );
    }
}

#[test]
fn invalid_selections_are_rejected() {
    // 缺字段 / 空串 / 非对象 / strict 未知键。
    for value in [
        serde_json::json!({"modelId": "m"}),
        serde_json::json!({"providerId": "p"}),
        serde_json::json!({"providerId": "  ", "modelId": "m"}),
        serde_json::json!([]),
        serde_json::json!({"providerId": "p", "modelId": "m", "extra": 1}),
        serde_json::json!({"providerId": "p", "modelId": "m", "options": {"x": 1}}),
    ] {
        assert!(normalize(Some(&value)).is_err(), "应拒绝: {value}");
    }
}

#[test]
fn absent_or_null_means_clear() {
    assert_eq!(normalize(None).unwrap(), None);
    assert_eq!(normalize(Some(&serde_json::Value::Null)).unwrap(), None);
}
