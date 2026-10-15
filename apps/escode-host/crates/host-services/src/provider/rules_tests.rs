//! 规则引擎单测（语义对照 TS `ModelConfigRules`）。

use serde_json::json;

use super::*;

fn input<'a>() -> ResolveInput<'a> {
    ResolveInput {
        provider_id: "p",
        template_id: Some("t"),
        model_id: "GLM-5.1",
        api_type: Some("anthropic-messages"),
        base_url: Some("https://API.example.com:443/v1/"),
    }
}

#[test]
fn layers_apply_in_order_and_later_rules_win() {
    let builtin = builtin_rules(&json!({
        "modelRules":[{"modelMatch":".*glm-5.*","config":{"enabled":true,"properties":{"contextWindow":100}}}],
        "modelApiRules":[{"modelMatch":".*","apiTypeMatch":"anthropic-messages","config":{"properties":{"contextWindow":200}}}],
        "providerSiteRules":[{"modelMatch":".*","baseUrlMatch":"https://api\\.example\\.com/v1","config":{"properties":{"supportsToolCall":true}}}],
        "templateModelRules":[{"templateId":"t","modelId":"GLM-5.1","config":{"properties":{"contextWindow":300}}}],
        "builtinProviderModelRules":[{"providerId":"p","modelId":"GLM-5.1","config":{"enabled":false}}],
    }));
    let refs: Vec<&Rule> = builtin.iter().collect();
    let resolved = resolve(&refs, &input());
    assert_eq!(resolved["properties"]["contextWindow"], 300);
    assert_eq!(
        resolved["properties"]["supportsToolCall"], true,
        "baseUrl 规范化（大小写/默认端口/尾斜杠）后命中"
    );
    assert_eq!(resolved["enabled"], false);
}

#[test]
fn model_match_is_case_insensitive_but_api_and_url_are_not() {
    let rules = builtin_rules(&json!({
        "modelRules":[{"modelMatch":"glm-5\\.1","config":{"enabled":true}}],
        "modelApiRules":[{"modelMatch":".*","apiTypeMatch":"ANTHROPIC-MESSAGES","config":{"properties":{"contextWindow":1}}}],
    }));
    let refs: Vec<&Rule> = rules.iter().collect();
    let resolved = resolve(&refs, &input());
    assert_eq!(resolved["enabled"], true);
    assert!(resolved.get("properties").is_none());
}

#[test]
fn manual_rule_clears_open_leaves_but_keeps_system_leaves() {
    let builtin = builtin_rules(&json!({"modelRules":[{"modelMatch":".*","config":{
        "enabled":true,
        "properties":{"contextWindow":1,"supportsToolCall":true,"inputFormat":{"supportsText":true,"supportsImage":true}},
        "optionSpecs":{"reasoningLevel":{"values":["a"],"map":"m"},"maxOutputTokens":{"max":9,"map":"mm"}}}}]}));
    let personal = personal_rules(
        &json!({"manualProviderModelRules":[{"providerId":"p","modelId":"GLM-5.1","config":{
        "enabled":true,"properties":{"contextWindow":50},"optionSpecs":{"maxOutputTokens":{"max":7}}}}]}),
    );
    let all: Vec<&Rule> = builtin.iter().chain(personal.iter()).collect();
    let resolved = resolve(&all, &input());
    assert_eq!(resolved["properties"]["contextWindow"], 50);
    assert_eq!(resolved["properties"]["supportsToolCall"], true);
    assert_eq!(
        resolved["properties"]["inputFormat"],
        json!({"supportsText":true})
    );
    assert!(
        resolved["optionSpecs"].get("reasoningLevel").is_none(),
        "reasoningLevel 整体清除"
    );
    assert_eq!(
        resolved["optionSpecs"]["maxOutputTokens"],
        json!({"max":7,"map":"mm"})
    );
}

#[test]
fn base_url_normalization_strips_trailing_slashes_but_keeps_query() {
    assert_eq!(
        normalize_base_url("https://A.com:443/v1//").as_deref(),
        Some("https://a.com/v1")
    );
    assert_eq!(
        normalize_base_url("http://x.com/a/?k=V#f").as_deref(),
        Some("http://x.com/a?k=V#f")
    );
    assert_eq!(normalize_base_url("not a url"), None);
}
