//! `provider-settings.resolveModelConfig`（TS `ProviderSettingsFacade.resolveModelConfig`）：只读预览。
//!
//! 不带 `personalConfig`：按当前规则合成并解析；带 `personalConfig`：在内存里把草稿当作该 Model 的智能规则
//! （必要时先迁移改名前的规则）再合成，`inheritedConfig` 只含 Built-in 规则的结果。

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use super::issues::model_issues;
use super::personal::{Doc, exact_from_rules, rules_from_exact};
use super::rules::{ResolveInput, Rule, resolve};
use super::{ProviderService, sparse};

fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or_default()
}

impl ProviderService {
    pub async fn resolve_model_config(&self, input: &Value) -> Result<Value> {
        let loaded = self.load().await?;
        let provider_id = text(input, "providerId");
        let model_id = text(input, "modelId");
        let provider = loaded
            .effective
            .get(provider_id)
            .ok_or_else(|| anyhow!("Provider 不存在: {provider_id}"))?;
        let template_id = provider.template_id.as_ref().and_then(Value::as_str);
        let api = provider.config.get("api");
        let resolve_input = ResolveInput {
            provider_id,
            template_id,
            model_id,
            api_type: api.and_then(|a| a.get("type")).and_then(Value::as_str),
            base_url: api.and_then(|a| a.get("baseUrl")).and_then(Value::as_str),
        };
        let compose = |personal: &[Rule]| -> Value {
            let rules: Vec<&Rule> = loaded.builtin_rules.iter().chain(personal.iter()).collect();
            resolve(&rules, &resolve_input)
        };
        let path = ["providers", provider_id, "models", model_id].map(str::to_owned);

        let (inherited, effective) = match input.get("personalConfig") {
            None => {
                let config = compose(&loaded.personal_rules);
                (config.clone(), config)
            }
            Some(personal_config) => {
                sparse::model_config(personal_config).map_err(|message| anyhow!(message))?;
                let builtin_only: Vec<&Rule> = loaded.builtin_rules.iter().collect();
                let inherited = resolve(&builtin_only, &resolve_input);
                let mut doc = Doc {
                    exact: exact_from_rules(&loaded.personal_rules),
                    ..Doc::default()
                };
                let original = text(input, "originalModelId");
                if original != model_id {
                    doc.rename_exact_model(provider_id, original, model_id);
                }
                // 草稿预览走智能配置：固定模式不请求推荐，重新开启时不沿用旧固定标记。
                doc.set_exact(provider_id, model_id, personal_config.clone(), Some(true))
                    .map_err(|message| anyhow!(message))?;
                (inherited, compose(&rules_from_exact(&doc.exact)))
            }
        };
        Ok(json!({
            "inheritedConfig": inherited,
            "effectiveConfig": effective,
            "issues": model_issues(&effective, &path),
        }))
    }
}
