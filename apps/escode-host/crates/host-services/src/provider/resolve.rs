//! Registry 解析（TS `ProviderConfigResolver.resolve` 的 `registryProviders` 部分）。
//! 层次：Built-in →（Account 覆盖）→ Template → Personal；模型规则 = Built-in 全部层 + Personal 精确规则。

use serde_json::{Map, Value, json};

use super::issues::{model_issues, provider_issues};
use super::overlay::{Kind, overlay, without_group};
use super::rules::{ResolveInput, Rule, resolve};
pub use super::types::{
    ProviderMap, ProviderRule, RegistryProvider, Resolution, ResolvedModel, ResolvedProvider,
    Template,
};

pub struct Input<'a> {
    pub builtin_providers: &'a ProviderMap,
    pub templates: &'a [Template],
    pub personal_providers: &'a ProviderMap,
    pub builtin_rules: &'a [Rule],
    pub personal_rules: &'a [Rule],
    pub account_providers: &'a ProviderMap,
    /// 账号连接状态（`states[providerId].current`）；P1 没有账号源，恒为空。
    pub account_current: &'a dyn Fn(&str) -> Option<bool>,
    pub personal_provider_order: &'a [String],
}

fn unique_in_order(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

/// TS `resolveOwnedOrder`。
pub fn resolve_owned_order(
    builtin: &[String],
    personal: &[String],
    requested: &[String],
) -> Vec<String> {
    let builtin = unique_in_order(builtin);
    let personal: Vec<String> = unique_in_order(personal)
        .into_iter()
        .filter(|id| !builtin.contains(id))
        .collect();
    let ordered: Vec<String> = unique_in_order(requested)
        .into_iter()
        .filter(|id| builtin.contains(id) || personal.contains(id))
        .collect();
    builtin
        .iter()
        .filter(|id| !ordered.contains(id))
        .chain(ordered.iter())
        .chain(personal.iter().filter(|id| !ordered.contains(id)))
        .cloned()
        .collect()
}

fn group_of(config: &Value) -> Option<&str> {
    config.get("group").and_then(Value::as_str)
}

fn string_list(config: &Value, key: &str) -> Vec<String> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// 解析出进入 Registry 的 Provider（顺序 = Registry 顺序）。
pub fn resolve_registry(input: &Input<'_>) -> Resolution {
    let account = ProviderMap(
        input
            .account_providers
            .0
            .iter()
            .filter(|rule| input.builtin_providers.has(&rule.provider_id))
            .map(|rule| ProviderRule {
                config: without_group(&rule.config),
                ..rule.clone()
            })
            .collect(),
    );
    let concrete = input.builtin_providers.overlay(&account);
    let template_config = |rule: &ProviderRule| -> Option<&Value> {
        let id = rule.template_id.as_ref()?.as_str()?;
        input
            .templates
            .iter()
            .find(|t| t.template_id == id)
            .map(|t| &t.config)
    };
    let effective_builtin = concrete.map_configs(|config, rule| match template_config(rule) {
        Some(template) => overlay(Kind::Provider, template, config),
        None => config.clone(),
    });
    let personal = input.personal_providers.map_configs(|config, rule| {
        if effective_builtin.has(&rule.provider_id) {
            without_group(config)
        } else {
            config.clone()
        }
    });
    let template_personal = personal.map_configs(|config, rule| {
        if effective_builtin.has(&rule.provider_id) {
            return config.clone();
        }
        match template_config(rule) {
            Some(template) => overlay(Kind::Provider, template, config),
            None => config.clone(),
        }
    });
    let effective = effective_builtin.overlay(&template_personal);

    let rules: Vec<&Rule> = input
        .builtin_rules
        .iter()
        .chain(input.personal_rules.iter())
        .collect();

    // Provider 顺序：family 在前，其余 Built-in / Personal 按拥有者顺序。
    let family: Vec<String> = effective
        .0
        .iter()
        .filter(|rule| {
            matches!(
                group_of(&rule.config),
                Some("zai-family" | "bigmodel-family")
            )
        })
        .map(|rule| rule.provider_id.clone())
        .collect();
    let builtin_ids: Vec<String> = input
        .builtin_providers
        .keys()
        .into_iter()
        .filter(|id| !family.contains(id))
        .collect();
    let personal_ids: Vec<String> = input
        .personal_providers
        .keys()
        .into_iter()
        .filter(|id| !builtin_ids.contains(id) && !family.contains(id))
        .collect();
    let order: Vec<String> = family
        .iter()
        .cloned()
        .chain(resolve_owned_order(
            &builtin_ids,
            &personal_ids,
            input.personal_provider_order,
        ))
        .collect();

    let mut result = Vec::new();
    let mut resolved_providers = Vec::new();
    for provider_id in order {
        let Some(rule) = effective.get(&provider_id) else {
            continue;
        };
        let config = &rule.config;
        let account_access =
            config.pointer("/access/type").and_then(Value::as_str) == Some("zhipu-account");
        // 账号不再支持总禁用。
        let enabled = account_access
            || rule
                .enabled
                .as_ref()
                .and_then(Value::as_bool)
                .unwrap_or(true);
        let template_id = rule.template_id.as_ref().and_then(Value::as_str);
        let provider_path = vec!["providers".to_owned(), provider_id.clone()];
        let mut provider_issues = provider_issues(config, &provider_path);
        let template_cfg = template_config(rule);
        if let Some(id) = template_id
            && template_cfg.is_none()
        {
            provider_issues.push(json!({
                "code": "missing-template",
                "path": [&provider_path[0], &provider_path[1], "templateId"],
                "message": format!("Provider Template 不存在: {id}"),
            }));
        }
        let builtin_ids_in_order = unique_in_order(&string_list(config, "builtinModelIds"));
        let personal_ids_in_order: Vec<String> =
            unique_in_order(&string_list(config, "personalModelIds"))
                .into_iter()
                .filter(|id| !builtin_ids_in_order.contains(id))
                .collect();
        let ordered = resolve_owned_order(
            &builtin_ids_in_order,
            &personal_ids_in_order,
            &string_list(config, "modelOrder"),
        );
        let entitled = !account_access
            || config.pointer("/access/entitled").and_then(Value::as_bool) == Some(true);
        let current = (input.account_current)(&provider_id) != Some(false);
        let provider_executable = enabled && entitled && current && provider_issues.is_empty();
        let api_type = config.pointer("/api/type").and_then(Value::as_str);
        let base_url = config.pointer("/api/baseUrl").and_then(Value::as_str);
        let mut models = Vec::new();
        for model_id in &ordered {
            let resolve_input = ResolveInput {
                provider_id: &provider_id,
                template_id,
                model_id,
                api_type,
                base_url,
            };
            let model_config = resolve(&rules, &resolve_input);
            let builtin_only: Vec<&Rule> = input.builtin_rules.iter().collect();
            let effective_builtin_config = resolve(&builtin_only, &resolve_input);
            let mut model_path = provider_path.clone();
            model_path.extend(["models".to_owned(), model_id.clone()]);
            let issues = model_issues(&model_config, &model_path);
            let model_enabled = model_config.get("enabled").and_then(Value::as_bool) == Some(true);
            let executable = provider_executable && model_enabled && issues.is_empty();
            let selectable =
                executable && config.get("visibility").and_then(Value::as_str) != Some("hidden");
            models.push(ResolvedModel {
                model_id: model_id.clone(),
                builtin: builtin_ids_in_order.contains(model_id),
                config: model_config,
                effective_builtin_config,
                issues,
                enabled: model_enabled,
                executable,
                selectable,
            });
        }
        let executable_models: Vec<(String, Value)> = models
            .iter()
            .filter(|m| m.executable)
            .map(|m| (m.model_id.clone(), m.config.clone()))
            .collect();
        if provider_issues.is_empty() && !executable_models.is_empty() {
            result.push(RegistryProvider {
                provider_id: provider_id.clone(),
                provider_name: rule.provider_name.clone(),
                template_id: template_id.map(str::to_owned),
                config: config.clone(),
                models: executable_models,
            });
        }
        resolved_providers.push(ResolvedProvider {
            provider_id: provider_id.clone(),
            provider_name: rule.provider_name.clone(),
            template_id: template_id.map(str::to_owned),
            enabled,
            config: config.clone(),
            template_config: template_cfg.cloned(),
            effective_builtin_config: effective_builtin
                .get(&provider_id)
                .map(|r| r.config.clone()),
            issues: provider_issues,
            models,
        });
    }
    Resolution {
        registry: result,
        effective,
        resolved: resolved_providers,
    }
}

/// 账号源缺席时的 fail-closed Overlay（TS `createFailClosedAccountProviderConfigSnapshot`）：
/// 每个 `zhipu-account` 的 Built-in Provider 标记 `entitled:false`。
pub fn fail_closed_account(builtin: &ProviderMap) -> ProviderMap {
    ProviderMap(
        builtin
            .0
            .iter()
            .filter(|rule| {
                rule.config.pointer("/access/type").and_then(Value::as_str) == Some("zhipu-account")
            })
            .map(|rule| {
                let mut access = Map::new();
                access.insert("type".into(), Value::String("zhipu-account".into()));
                access.insert("entitled".into(), Value::Bool(false));
                let mut config = Map::new();
                config.insert("access".into(), Value::Object(access));
                ProviderRule {
                    provider_id: rule.provider_id.clone(),
                    config: Value::Object(config),
                    ..ProviderRule::default()
                }
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
