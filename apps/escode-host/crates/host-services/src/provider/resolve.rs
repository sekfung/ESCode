//! Registry 解析（TS `ProviderConfigResolver.resolve` 的 `registryProviders` 部分）。
//! 层次：Built-in →（Account 覆盖）→ Template → Personal；模型规则 = Built-in 全部层 + Personal 精确规则。

use serde_json::{Map, Value};

use super::complete::{model_complete, provider_complete};
use super::overlay::{Kind, overlay, without_group};
use super::rules::{ResolveInput, Rule, resolve};

/// TS `ProviderConfigRule`：`None` = `undefined`（继承），`Some(Null)` = 显式 null。
#[derive(Clone, Debug, Default)]
pub struct ProviderRule {
    pub provider_id: String,
    pub template_id: Option<Value>,
    pub provider_name: Option<Value>,
    pub enabled: Option<Value>,
    pub config: Value,
}

#[derive(Clone, Debug)]
pub struct Template {
    pub template_id: String,
    pub config: Value,
}

/// 保持插入序的 Provider 规则表（TS `ProviderConfigMap`）。
#[derive(Clone, Debug, Default)]
pub struct ProviderMap(pub Vec<ProviderRule>);

impl ProviderMap {
    pub fn get(&self, id: &str) -> Option<&ProviderRule> {
        self.0.iter().find(|rule| rule.provider_id == id)
    }

    pub fn has(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub fn keys(&self) -> Vec<String> {
        self.0.iter().map(|rule| rule.provider_id.clone()).collect()
    }

    /// TS `overlay`：已有的就地叠加（位置不变），新的追加。
    fn overlay(&self, next: &ProviderMap) -> ProviderMap {
        let mut rules = self.0.clone();
        for rule in &next.0 {
            match rules
                .iter_mut()
                .find(|current| current.provider_id == rule.provider_id)
            {
                Some(current) => {
                    let keep = |base: &Option<Value>, incoming: &Option<Value>| {
                        incoming.clone().or_else(|| base.clone())
                    };
                    current.template_id = keep(&current.template_id, &rule.template_id);
                    current.provider_name = keep(&current.provider_name, &rule.provider_name);
                    current.enabled = keep(&current.enabled, &rule.enabled);
                    current.config = overlay(Kind::Provider, &current.config, &rule.config);
                }
                None => rules.push(rule.clone()),
            }
        }
        ProviderMap(rules)
    }

    fn map_configs(&self, transform: impl Fn(&Value, &ProviderRule) -> Value) -> ProviderMap {
        ProviderMap(
            self.0
                .iter()
                .map(|rule| ProviderRule {
                    config: transform(&rule.config, rule),
                    ..rule.clone()
                })
                .collect(),
        )
    }
}

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

#[derive(Debug)]
pub struct RegistryProvider {
    pub provider_id: String,
    pub provider_name: Option<Value>,
    pub template_id: Option<String>,
    pub config: Value,
    pub models: Vec<(String, Value)>,
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

/// 解析结果：Registry 与生效的 Provider 规则表（旧档位回溯等需要读生效的 api / templateId）。
pub struct Resolution {
    pub registry: Vec<RegistryProvider>,
    pub effective: ProviderMap,
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
        let provider_ok = provider_complete(config);
        let template_missing = template_id.is_some() && template_config(rule).is_none();
        let provider_issues = !provider_ok || template_missing;
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
        let provider_executable = enabled && entitled && current && !provider_issues;
        if provider_issues {
            continue;
        }
        let api_type = config.pointer("/api/type").and_then(Value::as_str);
        let base_url = config.pointer("/api/baseUrl").and_then(Value::as_str);
        let mut models = Vec::new();
        for model_id in ordered {
            let model_config = resolve(
                &rules,
                &ResolveInput {
                    provider_id: &provider_id,
                    template_id,
                    model_id: &model_id,
                    api_type,
                    base_url,
                },
            );
            let model_ok = model_complete(&model_config);
            let model_enabled = model_config.get("enabled").and_then(Value::as_bool) == Some(true);
            if provider_executable && model_enabled && model_ok {
                models.push((model_id, model_config));
            }
        }
        if models.is_empty() {
            continue;
        }
        result.push(RegistryProvider {
            provider_id,
            provider_name: rule.provider_name.clone(),
            template_id: template_id.map(str::to_owned),
            config: config.clone(),
            models,
        });
    }
    Resolution {
        registry: result,
        effective,
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
