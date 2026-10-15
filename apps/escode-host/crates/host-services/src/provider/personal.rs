//! Personal 配置文档的可写模型（TS `ProviderConfigLayerUpdate` + `encodeProviderConfigFile`）。
//!
//! 与只读解析（[`super::files::parse_personal`]）共用同一份结构校验：写出前把编码结果再解码一遍
//! （TS `#writeLocked` 的 `decode(encode(update))`），任何越权/坏值在落盘前就失败。

use anyhow::Result;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::files::parse_personal;
use super::overlay::{Kind, overlay};
use super::resolve::{ProviderMap, ProviderRule};
use super::rules::{Rule, RuleKind, personal_rules};

/// TS `ModelConfigRule` 的精确规则（Personal 文件只有这两种）。
#[derive(Clone, Debug)]
pub struct ExactRule {
    /// `true` = `manual-provider-model`；`false` = `provider-model`。
    pub manual: bool,
    pub provider_id: String,
    pub model_id: String,
    pub config: Value,
}

/// 一份 Personal 配置。`provider_order = None` 表示文件没有声明该键（与空数组不同，影响 revision）。
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub providers: ProviderMap,
    pub exact: Vec<ExactRule>,
    pub provider_order: Option<Vec<String>>,
    pub default_selection: Option<Value>,
}

impl Doc {
    /// 文件不存在时的空配置（TS `emptyUpdate`：`providerOrder: []`，影响 revision）。
    pub fn empty() -> Doc {
        Doc {
            provider_order: Some(Vec::new()),
            ..Doc::default()
        }
    }

    /// 严格解码；与 TS `decodeProviderConfigFile` 一样，无效文件整体报错。
    pub fn decode(input: &Value) -> Result<Doc> {
        let parsed = parse_personal(input)?;
        let exact = exact_from_rules(&parsed.rules);
        let provider_order = input["config"]
            .get("providerOrder")
            .map(|_| parsed.provider_order);
        Ok(Doc {
            providers: parsed.providers,
            exact,
            provider_order,
            default_selection: parsed
                .default_selection
                .map(|selection| selection.to_value()),
        })
    }

    /// TS `encodeProviderConfigFile`：智能规则与手动规则分组输出，组内保持相对顺序。
    pub fn encode(&self) -> Value {
        let rules = |manual: bool| -> Vec<Value> {
            self.exact
                .iter()
                .filter(|rule| rule.manual == manual)
                .map(|rule| {
                    json!({"providerId": rule.provider_id, "modelId": rule.model_id, "config": rule.config})
                })
                .collect()
        };
        let mut config = Map::new();
        if let Some(order) = &self.provider_order {
            config.insert("providerOrder".into(), json!(order));
        }
        config.insert(
            "providerConfigRules".into(),
            json!({"providerRules": self.providers.0.iter().map(encode_provider).collect::<Vec<_>>()}),
        );
        config.insert(
            "modelConfigRules".into(),
            json!({"providerModelRules": rules(false), "manualProviderModelRules": rules(true)}),
        );
        if let Some(selection) = &self.default_selection {
            config.insert("defaultModelSelection".into(), selection.clone());
        }
        json!({"schemaVersion": 1, "config": Value::Object(config)})
    }

    /// 写盘前的规范化：编码 → 严格解码 → 再编码（越权/坏值在此失败，且 revision 与读回一致）。
    pub fn canonical(&self) -> Result<(Doc, Value)> {
        let decoded = Doc::decode(&self.encode())?;
        let encoded = decoded.encode();
        Ok((decoded, encoded))
    }

    /// 内容版本：规范编码的 SHA-256（TS `snapshotFromUpdate`）。
    pub fn revision(&self) -> String {
        revision_of(&self.encode())
    }

    /// TS `getExactRule`：最后一条同身份规则。
    pub fn exact_rule(&self, provider_id: &str, model_id: &str) -> Option<&ExactRule> {
        self.exact
            .iter()
            .rev()
            .find(|rule| rule.provider_id == provider_id && rule.model_id == model_id)
    }

    /// TS `getExact`：同身份规则按序叠加。
    pub fn exact_config(&self, provider_id: &str, model_id: &str) -> Option<Value> {
        self.exact
            .iter()
            .filter(|rule| rule.provider_id == provider_id && rule.model_id == model_id)
            .fold(None, |acc, rule| {
                Some(match acc {
                    Some(base) => overlay(Kind::Model, &base, &rule.config),
                    None => rule.config.clone(),
                })
            })
    }

    /// TS `setExact`：原位替换第一条同身份规则并去掉其余，没有则追加。
    pub fn set_exact(
        &mut self,
        provider_id: &str,
        model_id: &str,
        config: Value,
        use_recommended: Option<bool>,
    ) -> Result<(), String> {
        let manual = match use_recommended {
            None => self
                .exact_rule(provider_id, model_id)
                .is_some_and(|rule| rule.manual),
            Some(recommended) => !recommended,
        };
        if provider_id.is_empty() || model_id.is_empty() {
            return Err("providerId/modelId 不能为空".to_owned());
        }
        if manual {
            super::sparse::manual_model_config(&config)?;
        } else {
            super::sparse::model_config(&config)?;
        }
        let replacement = ExactRule {
            manual,
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            config,
        };
        let mut result = Vec::with_capacity(self.exact.len() + 1);
        let mut replaced = false;
        for rule in self.exact.drain(..) {
            if rule.provider_id == provider_id && rule.model_id == model_id {
                if !replaced {
                    result.push(replacement.clone());
                }
                replaced = true;
            } else {
                result.push(rule);
            }
        }
        if !replaced {
            result.push(replacement);
        }
        self.exact = result;
        Ok(())
    }

    pub fn delete_exact(&mut self, provider_id: &str, model_id: &str) {
        self.exact
            .retain(|rule| rule.provider_id != provider_id || rule.model_id != model_id);
    }

    pub fn rename_exact_model(&mut self, provider_id: &str, current: &str, next: &str) {
        if current == next {
            return;
        }
        for rule in &mut self.exact {
            if rule.provider_id == provider_id && rule.model_id == current {
                rule.model_id = next.to_owned();
            }
        }
    }

    pub fn delete_exact_for_provider(&mut self, provider_id: &str) {
        self.exact.retain(|rule| rule.provider_id != provider_id);
    }
}

/// 解析后的 Personal 规则 → 可编辑的精确规则表。
pub fn exact_from_rules(rules: &[Rule]) -> Vec<ExactRule> {
    rules
        .iter()
        .map(|rule| ExactRule {
            manual: rule.kind == RuleKind::ManualProviderModel,
            provider_id: text(&rule.value, "providerId"),
            model_id: text(&rule.value, "modelId"),
            config: rule
                .value
                .get("config")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
        .collect()
}

/// 精确规则表 → 可参与规则合成的 `Rule`（智能规则先于手动规则，与文件分组一致）。
pub fn rules_from_exact(exact: &[ExactRule]) -> Vec<Rule> {
    let group = |manual: bool| -> Vec<Value> {
        exact
            .iter()
            .filter(|rule| rule.manual == manual)
            .map(|rule| {
                json!({"providerId": rule.provider_id, "modelId": rule.model_id, "config": rule.config})
            })
            .collect()
    };
    personal_rules(&json!({
        "providerModelRules": group(false),
        "manualProviderModelRules": group(true),
    }))
}

pub fn revision_of(encoded: &Value) -> String {
    let digest = Sha256::digest(
        serde_json::to_string(encoded)
            .unwrap_or_default()
            .as_bytes(),
    );
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// TS `ProviderConfigMap.toJSON()` 的一条：`undefined` 字段整键省略，`null` 保留。
fn encode_provider(rule: &ProviderRule) -> Value {
    let mut out = Map::new();
    out.insert("providerId".into(), json!(rule.provider_id));
    for (key, value) in [
        ("templateId", &rule.template_id),
        ("providerName", &rule.provider_name),
        ("enabled", &rule.enabled),
    ] {
        if let Some(value) = value {
            out.insert(key.into(), value.clone());
        }
    }
    out.insert("config".into(), rule.config.clone());
    Value::Object(out)
}

#[cfg(test)]
#[path = "personal_tests.rs"]
mod tests;
