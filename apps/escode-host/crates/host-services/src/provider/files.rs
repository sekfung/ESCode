//! 配置文件解码（TS `decodeESCodeBuiltinRelease` / `decodeProviderConfigFile` 的结构校验部分）。
//!
//! 校验到「键集合 / 版本 / 唯一性 / 枚举组」这一层；叶子字段的值域（URL、枚举、数值）不在这里拒绝整份文件，
//! 而由完整性判定把单个 Provider / Model 排除出 Registry（与 TS 的差异登记在 spec §6）。

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use super::resolve::{ProviderMap, ProviderRule, Template};
use super::rules::{Rule, builtin_rules, personal_rules};
use super::selection::Selection;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{what} 必须是对象"))
}

fn strict(map: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<()> {
    if let Some(extra) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("{what} 含未知字段 {extra}");
    }
    Ok(())
}

fn array<'a>(map: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a Vec<Value>> {
    map.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{what}.{key} 必须是数组"))
}

/// TS `freezeModelIds`：每个 ID trim，空串报错。
fn normalize_model_ids(config: &mut Map<String, Value>) -> Result<()> {
    for key in ["builtinModelIds", "personalModelIds", "modelOrder"] {
        if let Some(Value::Array(items)) = config.get_mut(key) {
            for item in items.iter_mut() {
                if let Some(text) = item.as_str() {
                    let trimmed = crate::git::paths::js_trim(text);
                    if trimmed.is_empty() {
                        bail!("Model ID 不能为空");
                    }
                    *item = Value::String(trimmed.to_owned());
                }
            }
        }
    }
    Ok(())
}

fn provider_rule(value: &Value, allowed_config: &[&str], what: &str) -> Result<ProviderRule> {
    let map = object(value, what)?;
    strict(
        map,
        &[
            "providerId",
            "templateId",
            "providerName",
            "enabled",
            "config",
        ],
        what,
    )?;
    let provider_id = map
        .get("providerId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{what}.providerId 不能为空"))?;
    let mut config = object(
        map.get("config").unwrap_or(&Value::Null),
        &format!("{what}.config"),
    )?
    .clone();
    strict(&config, allowed_config, &format!("{what}.config"))?;
    normalize_model_ids(&mut config)?;
    Ok(ProviderRule {
        provider_id: provider_id.to_owned(),
        template_id: map.get("templateId").cloned(),
        provider_name: map.get("providerName").cloned(),
        enabled: map.get("enabled").cloned(),
        config: Value::Object(config),
    })
}

fn unique_ids(rules: &[ProviderRule], what: &str) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for rule in rules {
        if !seen.insert(&rule.provider_id) {
            bail!("{what} 重复 providerId: {}", rule.provider_id);
        }
    }
    Ok(())
}

pub struct Builtin {
    pub providers: ProviderMap,
    pub templates: Vec<Template>,
    pub rules: Vec<Rule>,
}

/// TS `decodeESCodeBuiltinRelease`。
pub fn parse_builtin(input: &Value) -> Result<Builtin> {
    let root = object(input, "Built-in Release")?;
    strict(
        root,
        &["schemaVersion", "revision", "config"],
        "Built-in Release",
    )?;
    if root.get("schemaVersion") != Some(&Value::from(1)) {
        bail!("Built-in Release schemaVersion 必须是 1");
    }
    // revision 只校验值域；Registry 的变化检测基于内容指纹（见 mod.rs），不依赖发布序号。
    root.get("revision")
        .and_then(Value::as_u64)
        .filter(|r| *r <= MAX_SAFE_INTEGER)
        .ok_or_else(|| anyhow::anyhow!("Built-in Release revision 必须是非负安全整数"))?;
    let config = object(
        root.get("config").unwrap_or(&Value::Null),
        "Built-in Release.config",
    )?;
    strict(
        config,
        &["providerConfigRules", "modelConfigRules"],
        "Built-in Release.config",
    )?;
    let provider_rules = object(
        config.get("providerConfigRules").unwrap_or(&Value::Null),
        "providerConfigRules",
    )?;
    strict(
        provider_rules,
        &["templateRules", "providerRules"],
        "providerConfigRules",
    )?;
    let mut providers = Vec::new();
    for rule in array(provider_rules, "providerRules", "providerConfigRules")? {
        providers.push(provider_rule(
            rule,
            &[
                "group",
                "logo",
                "access",
                "api",
                "builtinModelIds",
                "visibility",
            ],
            "providerRules[]",
        )?);
    }
    unique_ids(&providers, "providerRules")?;
    if providers
        .iter()
        .any(|rule| rule.provider_id == "builtin:zapi")
    {
        bail!("ESCode Built-in Release 包含已退出的 Provider: builtin:zapi");
    }
    for rule in &providers {
        if rule
            .config
            .get("group")
            .and_then(Value::as_str)
            .is_none_or(|g| g == "standard-personal")
        {
            bail!(
                "Built-in Provider {} 的 group 必须是 family 组",
                rule.provider_id
            );
        }
    }
    let mut templates = Vec::new();
    let mut template_ids = std::collections::HashSet::new();
    for template in array(provider_rules, "templateRules", "providerConfigRules")? {
        let map = object(template, "templateRules[]")?;
        strict(
            map,
            &["templateId", "templateNameMap", "config"],
            "templateRules[]",
        )?;
        let template_id = map
            .get("templateId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("templateId 不能为空"))?;
        if !template_ids.insert(template_id.to_owned()) {
            bail!("templateRules 重复 templateId: {template_id}");
        }
        let mut config = object(
            map.get("config").unwrap_or(&Value::Null),
            "templateRules[].config",
        )?
        .clone();
        strict(
            &config,
            &["logo", "access", "api", "builtinModelIds"],
            "templateRules[].config",
        )?;
        normalize_model_ids(&mut config)?;
        templates.push(Template {
            template_id: template_id.to_owned(),
            config: Value::Object(config),
        });
    }
    let model_rules = object(
        config.get("modelConfigRules").unwrap_or(&Value::Null),
        "modelConfigRules",
    )?;
    strict(
        model_rules,
        &[
            "modelRules",
            "modelApiRules",
            "providerSiteRules",
            "templateModelRules",
            "builtinProviderModelRules",
        ],
        "modelConfigRules",
    )?;
    for key in [
        "modelRules",
        "modelApiRules",
        "providerSiteRules",
        "templateModelRules",
        "builtinProviderModelRules",
    ] {
        array(model_rules, key, "modelConfigRules")?;
    }
    Ok(Builtin {
        providers: ProviderMap(providers),
        templates,
        rules: builtin_rules(&Value::Object(model_rules.clone())),
    })
}

#[derive(Default)]
pub struct Personal {
    pub providers: ProviderMap,
    pub rules: Vec<Rule>,
    pub provider_order: Vec<String>,
    pub default_selection: Option<Selection>,
}

/// zod `z.string().trim().min(1)`。
fn trimmed_nonempty(value: &Value) -> Option<String> {
    let trimmed = crate::git::paths::js_trim(value.as_str()?);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_selection(value: &Value) -> Result<Selection> {
    let map = object(value, "defaultModelSelection")?;
    strict(
        map,
        &["providerId", "modelId", "options"],
        "defaultModelSelection",
    )?;
    let provider_id = map.get("providerId").and_then(trimmed_nonempty);
    let model_id = map.get("modelId").and_then(trimmed_nonempty);
    let (Some(provider_id), Some(model_id)) = (provider_id, model_id) else {
        bail!("defaultModelSelection 的 providerId/modelId 不能为空");
    };
    let options = match map.get("options") {
        None => None,
        Some(options) => {
            let options = object(options, "defaultModelSelection.options")?;
            strict(
                options,
                &["reasoningLevel"],
                "defaultModelSelection.options",
            )?;
            let mut out = Map::new();
            if let Some(level) = options.get("reasoningLevel") {
                let Some(level) = trimmed_nonempty(level) else {
                    bail!("reasoningLevel 不能为空")
                };
                out.insert("reasoningLevel".into(), level.into());
            }
            Some(out)
        }
    };
    Ok(Selection {
        provider_id,
        model_id,
        options,
    })
}

/// TS `decodeProviderConfigFile`（严格；失败由调用方按“降级为空配置”处理）。
pub fn parse_personal(input: &Value) -> Result<Personal> {
    let root = object(input, "Provider Config")?;
    match root.get("schemaVersion") {
        None => bail!("Provider Config 缺少 schemaVersion"),
        Some(version) if version == &Value::from(1) => {}
        Some(other) => bail!("Provider Config schemaVersion {other} 不受支持"),
    }
    strict(root, &["schemaVersion", "config"], "Provider Config")?;
    let config = object(
        root.get("config").unwrap_or(&Value::Null),
        "Provider Config.config",
    )?;
    strict(
        config,
        &[
            "providerOrder",
            "providerConfigRules",
            "modelConfigRules",
            "defaultModelSelection",
        ],
        "Provider Config.config",
    )?;
    let provider_rules = object(
        config.get("providerConfigRules").unwrap_or(&Value::Null),
        "providerConfigRules",
    )?;
    strict(provider_rules, &["providerRules"], "providerConfigRules")?;
    let mut providers = Vec::new();
    for rule in array(provider_rules, "providerRules", "providerConfigRules")? {
        let parsed = provider_rule(
            rule,
            &[
                "group",
                "logo",
                "access",
                "api",
                "personalModelIds",
                "modelOrder",
                "visibility",
            ],
            "providerRules[]",
        )?;
        match parsed.config.get("group") {
            None | Some(Value::Null) => {}
            Some(Value::String(group)) if group == "standard-personal" => {}
            Some(_) => bail!("Personal Provider 的 group 只能是 standard-personal"),
        }
        if parsed.provider_id.starts_with("account:") && parsed.config.get("access").is_some() {
            bail!("固定 Account Provider 的 Access 只能由 ESCode Built-in Config 声明");
        }
        providers.push(parsed);
    }
    unique_ids(&providers, "providerRules")?;
    let model_rules = object(
        config.get("modelConfigRules").unwrap_or(&Value::Null),
        "modelConfigRules",
    )?;
    strict(
        model_rules,
        &["providerModelRules", "manualProviderModelRules"],
        "modelConfigRules",
    )?;
    let smart = array(model_rules, "providerModelRules", "modelConfigRules")?;
    let manual = array(model_rules, "manualProviderModelRules", "modelConfigRules")?;
    let identity = |rule: &Value| (rule["providerId"].clone(), rule["modelId"].clone());
    if manual
        .iter()
        .any(|m| smart.iter().any(|s| identity(s) == identity(m)))
    {
        bail!("同一 Provider/Model 不能同时声明智能和手动配置");
    }
    let provider_order = match config.get("providerOrder") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow::anyhow!("providerOrder 元素必须是非空字符串"))?,
        Some(_) => bail!("providerOrder 必须是数组"),
    };
    let default_selection = config
        .get("defaultModelSelection")
        .map(parse_selection)
        .transpose()?;
    Ok(Personal {
        providers: ProviderMap(providers),
        rules: personal_rules(&Value::Object(model_rules.clone())),
        provider_order,
        default_selection,
    })
}
