//! Provider 解析用到的数据类型（TS `ProviderConfigRule` / `ProviderConfigMap` / `ResolvedProvider` 等）。

use serde_json::Value;

use super::overlay::{Kind, overlay};

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
    pub name_map: Value,
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
    pub fn overlay(&self, next: &ProviderMap) -> ProviderMap {
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

    /// TS `setRule`：已有的原位替换（Map.set 不改位置），新的追加。
    pub fn set_rule(&mut self, rule: ProviderRule) {
        match self
            .0
            .iter_mut()
            .find(|current| current.provider_id == rule.provider_id)
        {
            Some(current) => *current = rule,
            None => self.0.push(rule),
        }
    }

    /// TS `set(id, config)`：沿用已有规则的外层身份与名称，只换 `config`。
    pub fn set_config(&mut self, id: &str, config: Value) {
        let mut rule = self.get(id).cloned().unwrap_or_else(|| ProviderRule {
            provider_id: id.to_owned(),
            ..ProviderRule::default()
        });
        rule.config = config;
        self.set_rule(rule);
    }

    pub fn delete(&mut self, id: &str) {
        self.0.retain(|rule| rule.provider_id != id);
    }

    pub fn map_configs(&self, transform: impl Fn(&Value, &ProviderRule) -> Value) -> ProviderMap {
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

#[derive(Debug)]
pub struct RegistryProvider {
    pub provider_id: String,
    pub provider_name: Option<Value>,
    pub template_id: Option<String>,
    pub config: Value,
    pub models: Vec<(String, Value)>,
}

/// 解析结果：Registry 与生效的 Provider 规则表（旧档位回溯等需要读生效的 api / templateId）。
pub struct Resolution {
    pub registry: Vec<RegistryProvider>,
    pub effective: ProviderMap,
    /// 含不可执行者的完整解析结果（Settings 视图用）。
    pub resolved: Vec<ResolvedProvider>,
}

/// TS `ResolvedProviderModel`。
pub struct ResolvedModel {
    pub model_id: String,
    pub builtin: bool,
    pub config: Value,
    pub effective_builtin_config: Value,
    pub issues: Vec<Value>,
    pub enabled: bool,
    pub executable: bool,
    pub selectable: bool,
}

/// TS `ResolvedProvider`。
pub struct ResolvedProvider {
    pub provider_id: String,
    pub provider_name: Option<Value>,
    pub template_id: Option<String>,
    pub enabled: bool,
    pub config: Value,
    pub template_config: Option<Value>,
    pub effective_builtin_config: Option<Value>,
    pub issues: Vec<Value>,
    pub models: Vec<ResolvedModel>,
}
