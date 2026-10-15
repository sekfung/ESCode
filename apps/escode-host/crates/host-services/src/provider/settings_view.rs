//! `provider-settings.getView` 的视图投影（TS `facades.ts` `createProviderSettingsView`）。

use serde_json::{Map, Value, json};

use super::overlay::{Kind, overlay};
use super::resolve::{ProviderMap, ResolvedProvider, Template};
use super::rules::{Rule, RuleKind};

pub struct ViewInput<'a> {
    pub revision: u64,
    pub templates: &'a [Template],
    pub personal_providers: &'a ProviderMap,
    pub personal_rules: &'a [Rule],
    pub registry_provider_ids: Vec<&'a str>,
    pub resolved: &'a [ResolvedProvider],
}

fn is_exact(rule: &Rule, provider_id: &str, model_id: &str) -> bool {
    matches!(
        rule.kind,
        RuleKind::ProviderModel | RuleKind::ManualProviderModel
    ) && rule.value["providerId"] == provider_id
        && rule.value["modelId"] == model_id
}

/// TS `personalModels.getExact`：同一 Provider/Model 的全部精确规则按序叠加。
fn exact_config(rules: &[Rule], provider_id: &str, model_id: &str) -> Option<Value> {
    rules
        .iter()
        .filter(|rule| is_exact(rule, provider_id, model_id))
        .map(|rule| rule.value["config"].clone())
        .reduce(|acc, next| overlay(Kind::Model, &acc, &next))
}

fn project_provider(input: &ViewInput<'_>, provider: &ResolvedProvider) -> Value {
    let mut out = Map::new();
    out.insert("providerId".into(), provider.provider_id.clone().into());
    if let Some(name) = &provider.provider_name {
        out.insert("providerName".into(), name.clone());
    }
    if let Some(template) = &provider.template_id {
        out.insert("templateId".into(), template.clone().into());
    }
    out.insert("enabled".into(), provider.enabled.into());
    out.insert(
        "executable".into(),
        input
            .registry_provider_ids
            .contains(&provider.provider_id.as_str())
            .into(),
    );
    if let Some(config) = &provider.template_config {
        out.insert("templateConfig".into(), config.clone());
    }
    if let Some(config) = &provider.effective_builtin_config {
        out.insert("effectiveBuiltinConfig".into(), config.clone());
    }
    if let Some(rule) = input.personal_providers.get(&provider.provider_id) {
        out.insert("personalConfig".into(), rule.config.clone());
    }
    out.insert("effectiveConfig".into(), provider.config.clone());
    out.insert("issues".into(), Value::Array(provider.issues.clone()));
    let models: Vec<Value> = provider
        .models
        .iter()
        .map(|model| {
            let mut m = Map::new();
            m.insert("kind".into(), "candidate".into());
            m.insert("modelId".into(), model.model_id.clone().into());
            m.insert("builtin".into(), model.builtin.into());
            m.insert(
                "effectiveBuiltinConfig".into(),
                model.effective_builtin_config.clone(),
            );
            if let Some(exact) =
                exact_config(input.personal_rules, &provider.provider_id, &model.model_id)
            {
                m.insert("personalExactConfig".into(), exact);
            }
            if let Some(rule) = input
                .personal_rules
                .iter()
                .rev()
                .find(|rule| is_exact(rule, &provider.provider_id, &model.model_id))
            {
                m.insert(
                    "useRecommendedConfig".into(),
                    (rule.kind != RuleKind::ManualProviderModel).into(),
                );
            }
            m.insert("effectiveConfig".into(), model.config.clone());
            m.insert("enabled".into(), model.enabled.into());
            m.insert("executable".into(), model.executable.into());
            m.insert("selectable".into(), model.selectable.into());
            m.insert("issues".into(), Value::Array(model.issues.clone()));
            Value::Object(m)
        })
        .collect();
    out.insert("models".into(), Value::Array(models));
    Value::Object(out)
}

/// 视图：不隐藏的 Provider（含不可执行者）+ 全部模板 + 个人 Provider 顺序。
pub fn build(input: &ViewInput<'_>) -> Value {
    let visible: Vec<&ResolvedProvider> = input
        .resolved
        .iter()
        .filter(|provider| {
            provider.config.get("visibility").and_then(Value::as_str) != Some("hidden")
        })
        .collect();
    json!({
        "revision": input.revision,
        "providerTemplates": input.templates.iter().map(|t| json!({
            "templateId": t.template_id,
            "templateNameMap": t.name_map,
            "config": t.config,
        })).collect::<Vec<_>>(),
        "providerOrder": visible
            .iter()
            .filter(|p| p.config.get("group").and_then(Value::as_str) == Some("standard-personal"))
            .map(|p| p.provider_id.clone())
            .collect::<Vec<_>>(),
        "providers": visible.iter().map(|p| project_provider(input, p)).collect::<Vec<_>>(),
    })
}
