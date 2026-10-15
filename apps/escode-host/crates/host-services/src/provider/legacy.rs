//! 旧档位回溯（TS `legacy-reasoning-level.ts` / `legacy-reasoning-level-renames.ts`）。

use serde_json::{Value, json};

use super::overlay::{Kind, overlay};
use super::resolve::ProviderMap;
use super::rules::{ResolveInput, Rule, RuleKind, resolve};
use super::selection::Selection;

const LEGACY_RENAMES: [(&str, &str); 25] = [
    (".*glm-5(?:[.\\-:/\\[].*)?", "off"),
    (".*GLM-5\\.2(?:[.\\-:/\\[].*)?", "nothink"),
    (".*GLM-5-Turbo(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-5\\.1(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-5\\.1-highspeed(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-5v-turbo(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-4\\.7(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-4\\.7-flashx(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-4\\.7-flash(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-4\\.6(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-4\\.5(?:[.\\-:/\\[].*)?", "off"),
    (".*glm-4\\.5-air(?:[.\\-:/\\[].*)?", "off"),
    (".*kimi-k2\\.7-code(?:[.\\-:/\\[].*)?", "off"),
    (".*kimi-k2\\.6(?:[.\\-:/\\[].*)?", "off"),
    (".*kimi-k2\\.5(?:[.\\-:/\\[].*)?", "off"),
    (".*deepseek-v4-flash(?:[.\\-:/\\[].*)?", "off"),
    (".*deepseek-v4-pro(?:[.\\-:/\\[].*)?", "off"),
    (".*qwen3\\.5-plus(?:[.\\-:/\\[].*)?", "off"),
    (".*qwen3\\.5-flash(?:[.\\-:/\\[].*)?", "off"),
    (".*qwen-plus(?:[.\\-:/\\[].*)?", "off"),
    (".*qwen-flash(?:[.\\-:/\\[].*)?", "off"),
    (".*qwen3-vl-plus(?:[.\\-:/\\[].*)?", "off"),
    (".*mimo-v2\\.5(?:[.\\-:/\\[].*)?", "off"),
    (".*mimo-v2\\.5-pro(?:[.\\-:/\\[].*)?", "off"),
    (".*mimo-v2-flash(?:[.\\-:/\\[].*)?", "off"),
];

/// 输入：完整 snapshot 所需的各部分。
pub struct LegacyContext<'a> {
    pub personal_rules: &'a [Rule],
    pub builtin_rules: &'a [Rule],
    pub effective: &'a ProviderMap,
}

pub fn resolve_legacy_level(context: &LegacyContext<'_>, selection: &Selection) -> Option<String> {
    let old_level = selection.reasoning_level()?;
    if old_level != "off" && old_level != "nothink" {
        return None;
    }
    // 个人 exact 规则（最后一条匹配者）若声明了 reasoningLevel 的 values/map 或是手动规则，不回溯。
    let personal = context.personal_rules.iter().rev().find(|rule| {
        rule.value["providerId"] == selection.provider_id.as_str()
            && rule.value["modelId"] == selection.model_id.as_str()
    });
    if let Some(rule) = personal {
        let spec = rule.value.pointer("/config/optionSpecs/reasoningLevel");
        if rule.kind == RuleKind::ManualProviderModel
            || spec.is_some_and(|s| s.get("values").is_some() || s.get("map").is_some())
        {
            return None;
        }
    }
    let provider = context.effective.get(&selection.provider_id)?;
    let renames = LEGACY_RENAMES;
    // 构造「旧值域」规则：Built-in model 规则里 values 含 disabled 且 modelMatch 精确命中改名清单者，disabled → 旧档位名。
    let old_rules: Vec<Rule> = context
        .builtin_rules
        .iter()
        .map(|rule| {
            let rename = (rule.kind == RuleKind::Model)
                .then(|| renames.iter().find(|(m, _)| rule.value["modelMatch"] == *m))
                .flatten();
            let values = rule
                .value
                .pointer("/config/optionSpecs/reasoningLevel/values")
                .and_then(Value::as_array);
            match (rename, values) {
                (Some((_, old)), Some(values)) if values.iter().any(|v| v == "disabled") => {
                    let replaced: Vec<Value> = values
                        .iter()
                        .map(|v| {
                            if v == "disabled" {
                                Value::String((*old).into())
                            } else {
                                v.clone()
                            }
                        })
                        .collect();
                    let patch = json!({"optionSpecs": {"reasoningLevel": {"values": replaced}}});
                    let merged = overlay(Kind::Model, &rule.value["config"], &patch);
                    rule.with_config(merged)
                }
                _ => rule.clone(),
            }
        })
        .collect();
    let refs: Vec<&Rule> = old_rules.iter().collect();
    let template = provider.template_id.as_ref().and_then(Value::as_str);
    let resolved = resolve(
        &refs,
        &ResolveInput {
            provider_id: &selection.provider_id,
            template_id: template,
            model_id: &selection.model_id,
            api_type: provider.config.pointer("/api/type").and_then(Value::as_str),
            base_url: provider
                .config
                .pointer("/api/baseUrl")
                .and_then(Value::as_str),
        },
    );
    resolved
        .pointer("/optionSpecs/reasoningLevel/values")
        .and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|v| v == old_level))
        .then(|| "disabled".to_owned())
}
