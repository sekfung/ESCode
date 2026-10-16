//! Model 规则引擎（TS `ModelConfigRules`：`composeEffective` / `resolve` / 手动规则清理）。

use escode_host_control::logging;
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value, json};

use super::overlay::{Kind, overlay};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuleKind {
    Model,
    ModelApi,
    ProviderSite,
    TemplateModel,
    ProviderModel,
    ManualProviderModel,
}

#[derive(Clone)]
pub struct Rule {
    pub kind: RuleKind,
    pub value: Value,
    model_match: Option<Option<Regex>>,
    api_match: Option<Option<Regex>>,
    url_match: Option<Option<Regex>>,
}

/// JS `new RegExp("^(?:" + pattern + ")$", flags)`；Rust `regex` 不支持的写法（前瞻/反向引用）→ `None`，该规则永不命中。
fn compile(rule: &Value, key: &str, ignore_case: bool) -> Option<Option<Regex>> {
    let pattern = rule.get(key)?.as_str()?;
    Some(
        RegexBuilder::new(&format!("^(?:{pattern})$"))
            .case_insensitive(ignore_case)
            .build()
            .map_err(|error| {
                logging::warn(
                    "provider",
                    format!("rule pattern unsupported {pattern:?}: {error}"),
                )
            })
            .ok(),
    )
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

impl Rule {
    /// 同一规则换一份 config（匹配器不变）。
    pub fn with_config(&self, config: Value) -> Rule {
        let mut value = self.value.clone();
        value["config"] = config;
        Rule {
            value,
            ..self.clone()
        }
    }
}

fn make(kind: RuleKind, value: &Value) -> Rule {
    Rule {
        kind,
        value: value.clone(),
        model_match: compile(value, "modelMatch", true),
        api_match: compile(value, "apiTypeMatch", false),
        url_match: compile(value, "baseUrlMatch", false),
    }
}

/// 一个 Built-in 规则集（按 TS `parseESCodeBuiltinModelConfigRules` 的分组顺序）。
pub fn builtin_rules(model_config_rules: &Value) -> Vec<Rule> {
    let groups = [
        ("modelRules", RuleKind::Model),
        ("modelApiRules", RuleKind::ModelApi),
        ("providerSiteRules", RuleKind::ProviderSite),
        ("templateModelRules", RuleKind::TemplateModel),
        ("builtinProviderModelRules", RuleKind::ProviderModel),
    ];
    groups
        .into_iter()
        .flat_map(|(key, kind)| {
            array(model_config_rules, key)
                .iter()
                .map(move |rule| make(kind, rule))
        })
        .collect()
}

/// 个人规则集：只有精确规则（先智能后手动，TS `parsePersonalModelConfigRules`）。
pub fn personal_rules(model_config_rules: &Value) -> Vec<Rule> {
    array(model_config_rules, "providerModelRules")
        .iter()
        .map(|rule| make(RuleKind::ProviderModel, rule))
        .chain(
            array(model_config_rules, "manualProviderModelRules")
                .iter()
                .map(|rule| make(RuleKind::ManualProviderModel, rule)),
        )
        .collect()
}

pub struct ResolveInput<'a> {
    pub provider_id: &'a str,
    pub template_id: Option<&'a str>,
    pub model_id: &'a str,
    pub api_type: Option<&'a str>,
    pub base_url: Option<&'a str>,
}

fn is_exact(kind: RuleKind) -> bool {
    matches!(
        kind,
        RuleKind::ProviderModel | RuleKind::ManualProviderModel
    )
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// TS `normalizeBaseURLForRuleMatch`：WHATWG 解析后去掉 path 末尾斜杠（query/hash 原样）。
pub fn normalize_base_url(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    let suffix = format!(
        "{}{}",
        parsed.query().map(|q| format!("?{q}")).unwrap_or_default(),
        parsed
            .fragment()
            .map(|f| format!("#{f}"))
            .unwrap_or_default()
    );
    let serialized = parsed.to_string();
    let endpoint = &serialized[..serialized.len() - suffix.len()];
    Some(format!("{}{suffix}", endpoint.trim_end_matches('/')))
}

/// TS `clearManualModelConfig`：删去手动规则开放的叶子，保留系统叶子（含 `maxOutputTokens.map`）。
pub fn clear_manual(config: &Value) -> Value {
    let Some(map) = config.as_object() else {
        return config.clone();
    };
    let mut result = map.clone();
    if let Some(properties) = result.get("properties").cloned() {
        match properties {
            Value::Object(mut props) => {
                for key in [
                    "contextWindow",
                    "supportsJsonSchemaOutput",
                    "supportsNativeWebSearch",
                    "supportsMidConversationSystem",
                ] {
                    props.remove(key);
                }
                match props.remove("inputFormat") {
                    Some(Value::Object(mut format)) => {
                        for key in ["supportsImage", "supportsVideo", "supportsPdf"] {
                            format.remove(key);
                        }
                        if !format.is_empty() {
                            props.insert("inputFormat".into(), Value::Object(format));
                        }
                    }
                    Some(Value::Null) | None => {}
                    Some(other) => {
                        props.insert("inputFormat".into(), other);
                    }
                }
                if props.is_empty() {
                    result.remove("properties");
                } else {
                    result.insert("properties".into(), Value::Object(props));
                }
            }
            Value::Null => {
                result.remove("properties");
            }
            _ => {}
        }
    }
    if let Some(specs) = result.get("optionSpecs").cloned() {
        match specs {
            Value::Object(mut specs) => {
                specs.remove("reasoningLevel");
                match specs.remove("maxOutputTokens") {
                    Some(Value::Object(mut limit)) => {
                        limit.remove("max");
                        if !limit.is_empty() {
                            specs.insert("maxOutputTokens".into(), Value::Object(limit));
                        }
                    }
                    Some(Value::Null) | None => {}
                    Some(other) => {
                        specs.insert("maxOutputTokens".into(), other);
                    }
                }
                if specs.is_empty() {
                    result.remove("optionSpecs");
                } else {
                    result.insert("optionSpecs".into(), Value::Object(specs));
                }
            }
            Value::Null => {
                result.remove("optionSpecs");
            }
            _ => {}
        }
    }
    Value::Object(result)
}

/// TS `ModelConfigRules.resolve`（规则按给定顺序叠加；调用方负责 `composeEffective` 的拼接）。
pub fn resolve(rules: &[&Rule], input: &ResolveInput<'_>) -> Value {
    let mut result = Value::Object(Map::new());
    let base_url = input.base_url.and_then(normalize_base_url);
    for rule in rules {
        let config = rule
            .value
            .get("config")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if is_exact(rule.kind) {
            if text(&rule.value, "providerId") != Some(input.provider_id)
                || text(&rule.value, "modelId") != Some(input.model_id)
            {
                continue;
            }
            if rule.kind == RuleKind::ManualProviderModel {
                result = clear_manual(&result);
            }
            result = overlay(Kind::Model, &result, &config);
            continue;
        }
        if rule.kind == RuleKind::TemplateModel {
            if text(&rule.value, "templateId") == input.template_id
                && input.template_id.is_some()
                && text(&rule.value, "modelId") == Some(input.model_id)
            {
                result = overlay(Kind::Model, &result, &config);
            }
            continue;
        }
        let matches = |regex: &Option<Option<Regex>>, value: &str| -> bool {
            matches!(regex, Some(Some(re)) if re.is_match(value))
        };
        if !matches(&rule.model_match, input.model_id) {
            continue;
        }
        if matches!(rule.kind, RuleKind::ModelApi | RuleKind::ProviderSite)
            && rule.api_match.is_some()
        {
            match input.api_type {
                Some(api_type) if matches(&rule.api_match, api_type) => {}
                _ => continue,
            }
        }
        if rule.kind == RuleKind::ProviderSite {
            match &base_url {
                Some(url) if matches(&rule.url_match, url) => {}
                _ => continue,
            }
        }
        result = overlay(Kind::Model, &result, &config);
    }
    result
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
