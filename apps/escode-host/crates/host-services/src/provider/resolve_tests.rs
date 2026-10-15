//! Registry 解析单测（行为对照 TS `ProviderConfigResolver`；与 TS 的整体差分在 `rustHostProvider.test.ts`）。

use serde_json::{Value, json};

use super::super::rules::{Rule, builtin_rules, personal_rules};
use super::*;

fn model_config(levels: &[&str]) -> Value {
    json!({
        "enabled": true,
        "properties": {
            "requiresMfjsToolSchema": false, "contextWindow": 1000,
            "inputFormat": {"supportsText": true, "supportsImage": false, "supportsVideo": false, "supportsAudio": false, "supportsPdf": false},
            "outputFormat": {"supportsText": true},
            "supportsToolCall": true, "supportsJsonSchemaOutput": false, "supportsNativeWebSearch": false, "supportsMidConversationSystem": false
        },
        "optionSpecs": {
            "reasoningLevel": {"values": levels, "map": "{'effort': reasoningLevel}"},
            "maxOutputTokens": {"max": 100, "map": "{'max_tokens': maxOutputTokens}"}
        }
    })
}

fn rule(id: &str, config: Value) -> ProviderRule {
    ProviderRule {
        provider_id: id.into(),
        config,
        ..ProviderRule::default()
    }
}

fn api(url: &str) -> Value {
    json!({"type": "openai-responses", "baseUrl": url})
}

struct Fixture {
    builtin: ProviderMap,
    templates: Vec<Template>,
    personal: ProviderMap,
    builtin_rules: Vec<Rule>,
    personal_rules: Vec<Rule>,
    account: ProviderMap,
    order: Vec<String>,
}

impl Fixture {
    fn run(&self) -> Vec<RegistryProvider> {
        let current = |_: &str| None;
        resolve_registry(&Input {
            builtin_providers: &self.builtin,
            templates: &self.templates,
            personal_providers: &self.personal,
            builtin_rules: &self.builtin_rules,
            personal_rules: &self.personal_rules,
            account_providers: &self.account,
            account_current: &current,
            personal_provider_order: &self.order,
        })
        .registry
    }
}

fn base() -> Fixture {
    let rules = builtin_rules(
        &json!({"modelRules": [{"modelMatch": ".*", "config": model_config(&["low", "high"])}]}),
    );
    Fixture {
        builtin: ProviderMap(vec![rule(
            "b1",
            json!({"group": "zai-family", "access": {"type": "api-key", "apiKey": "k"}, "api": api("https://b.example.com"), "builtinModelIds": ["m1"]}),
        )]),
        templates: Vec::new(),
        personal: ProviderMap::default(),
        builtin_rules: rules,
        personal_rules: Vec::new(),
        account: ProviderMap::default(),
        order: Vec::new(),
    }
}

fn ids(registry: &[RegistryProvider]) -> Vec<&str> {
    registry.iter().map(|p| p.provider_id.as_str()).collect()
}

#[test]
fn owned_order_keeps_unordered_builtin_before_requested_before_personal() {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        resolve_owned_order(&s(&["a", "b"]), &s(&["x", "y"]), &s(&["y", "a", "zzz"])),
        s(&["b", "y", "a", "x"])
    );
    assert_eq!(
        resolve_owned_order(&s(&["a", "a"]), &s(&["a", "p"]), &[]),
        s(&["a", "p"])
    );
}

#[test]
fn personal_provider_joins_after_builtin_and_needs_a_complete_config() {
    let mut fixture = base();
    fixture.personal = ProviderMap(vec![
        rule(
            "p1",
            json!({"group": "standard-personal", "access": {"type": "api-key", "apiKey": "x"}, "api": api("https://p.example.com"), "personalModelIds": ["pm"]}),
        ),
        rule(
            "incomplete",
            json!({"group": "standard-personal", "personalModelIds": ["pm"]}),
        ),
    ]);
    assert_eq!(ids(&fixture.run()), vec!["b1", "p1"]);
}

#[test]
fn disabled_provider_hidden_models_and_invalid_models_are_excluded() {
    let mut fixture = base();
    fixture.builtin.0[0].enabled = Some(json!(false));
    assert!(fixture.run().is_empty(), "禁用的 Provider 不进 Registry");
    let mut fixture = base();
    fixture.builtin_rules =
        builtin_rules(&json!({"modelRules": [{"modelMatch": "m1", "config": {"enabled": false}}]}));
    assert!(
        fixture.run().is_empty(),
        "无可执行模型的 Provider 不进 Registry"
    );
}

#[test]
fn template_supplies_defaults_but_the_builtin_value_wins() {
    let mut fixture = base();
    fixture.templates = vec![Template {
        template_id: "t".into(),
        config: json!({"api": {"type": "anthropic-messages", "baseUrl": "https://tpl.example.com"}, "builtinModelIds": ["m1", "tm"]}),
    }];
    fixture.builtin.0[0].template_id = Some(json!("t"));
    let registry = fixture.run();
    assert_eq!(
        registry[0].config["api"]["baseUrl"], "https://b.example.com",
        "Built-in 覆盖模板"
    );
    assert_eq!(registry[0].config["api"]["type"], "openai-responses");
    assert_eq!(registry[0].template_id.as_deref(), Some("t"));
}

#[test]
fn missing_template_blocks_the_provider() {
    let mut fixture = base();
    fixture.builtin.0[0].template_id = Some(json!("ghost"));
    assert!(fixture.run().is_empty());
}

#[test]
fn account_provider_is_fail_closed_until_entitled() {
    let mut fixture = base();
    fixture.builtin = ProviderMap(vec![rule(
        "account:zai-start-plan",
        json!({"group": "zai-family", "access": {"type": "zhipu-account", "accountType": "zai", "mode": "start-plan", "entitled": true},
               "api": api("https://acc.example.com"), "builtinModelIds": ["m1"]}),
    )]);
    fixture.account = fail_closed_account(&fixture.builtin);
    assert!(fixture.run().is_empty(), "fail-closed 覆盖 entitled=false");
    fixture.account = ProviderMap::default();
    assert_eq!(ids(&fixture.run()), vec!["account:zai-start-plan"]);
}

#[test]
fn personal_manual_rule_replaces_open_leaves() {
    let mut fixture = base();
    fixture.personal_rules = personal_rules(
        &json!({"providerModelRules": [{"providerId": "b1", "modelId": "m1", "config": {"enabled": false}}]}),
    );
    assert!(fixture.run().is_empty(), "个人精确规则可以禁用模型");
}
