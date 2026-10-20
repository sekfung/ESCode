//! 模型选择：校验 / 生效解析 / 初始推荐 / 视图投影
//! （TS `registry.ts` `validateModelSelectionOptions`、`effective-model-selection.ts`、`model-selection-config.ts`、
//! `facades.ts` `ModelSelectionFacade.getView` 与 `legacy-reasoning-level*.ts`）。

use serde_json::{Map, Value, json};

use super::resolve::RegistryProvider;

/// `ModelSelection`：`{providerId, modelId, options?: {reasoningLevel?}}`。
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub provider_id: String,
    pub model_id: String,
    pub options: Option<Map<String, Value>>,
}

impl Selection {
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_object()?;
        Some(Self {
            provider_id: map.get("providerId")?.as_str()?.to_owned(),
            model_id: map.get("modelId")?.as_str()?.to_owned(),
            options: map.get("options").and_then(Value::as_object).cloned(),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("providerId".into(), self.provider_id.clone().into());
        object.insert("modelId".into(), self.model_id.clone().into());
        if let Some(options) = &self.options {
            object.insert("options".into(), Value::Object(options.clone()));
        }
        Value::Object(object)
    }

    pub fn reasoning_level(&self) -> Option<&str> {
        self.options.as_ref()?.get("reasoningLevel")?.as_str()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Validation {
    Ok,
    ReasoningLevelMissing,
    ReasoningLevelNotSupported,
}

fn levels(model_config: &Value) -> Vec<&str> {
    model_config
        .pointer("/optionSpecs/reasoningLevel/values")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// TS `validateModelSelectionOptions`。
pub fn validate_options(model_config: &Value, selection: &Selection) -> Validation {
    match selection.reasoning_level() {
        None => Validation::ReasoningLevelMissing,
        Some(level) if levels(model_config).contains(&level) => Validation::Ok,
        Some(_) => Validation::ReasoningLevelNotSupported,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Ordinary,
    AccountPlan,
}

/// TS `createNodeModelSelectionFacade` 的分类器。
pub fn classify(provider_id: &str) -> ProviderKind {
    const START_PLAN: [&str; 2] = ["account:zai-start-plan", "account:bigmodel-start-plan"];
    const BUILTIN: [&str; 6] = [
        "account:zai-individual-coding-plan",
        "account:zai-team-coding-plan",
        "account:zai-start-plan",
        "account:bigmodel-individual-coding-plan",
        "account:bigmodel-team-coding-plan",
        "account:bigmodel-start-plan",
    ];
    if START_PLAN.contains(&provider_id) {
        ProviderKind::Ordinary
    } else if BUILTIN.contains(&provider_id) {
        ProviderKind::AccountPlan
    } else {
        ProviderKind::Ordinary
    }
}

pub struct Effective {
    pub selection: Option<Selection>,
    pub issue: Option<&'static str>,
}

impl Effective {
    fn none(issue: &'static str) -> Self {
        Self {
            selection: None,
            issue: Some(issue),
        }
    }
}

/// 旧档位回溯函数（TS `resolveLegacyReasoningLevel` 的闭包形态）。
pub type LegacyLookup<'a> = &'a dyn Fn(&Selection) -> Option<String>;

/// TS `resolveEffectiveModelSelection`。`account_current`：账号连接状态里 `current === true` 的 Provider id（P1 恒空）。
pub fn resolve_effective(
    selection: Option<&Selection>,
    registry: &[RegistryProvider],
    account_current: &[String],
    classify_provider: &dyn Fn(&str) -> ProviderKind,
    legacy: Option<LegacyLookup<'_>>,
) -> Effective {
    let Some(original) = selection else {
        return Effective::none("selection-missing");
    };
    let kind = classify_provider(&original.provider_id);
    let mut provider_id = original.provider_id.clone();
    if kind == ProviderKind::AccountPlan {
        let current: Vec<&String> = account_current
            .iter()
            .filter(|id| classify_provider(id) == ProviderKind::AccountPlan)
            .collect();
        if current.len() != 1 {
            return Effective::none("account-connection-unavailable");
        }
        provider_id = current[0].clone();
    }
    let Some(provider) = registry.iter().find(|p| p.provider_id == provider_id) else {
        return Effective::none("provider-not-found");
    };
    if provider.config.get("visibility").and_then(Value::as_str) == Some("hidden") {
        return Effective::none("provider-not-found");
    }
    let Some((_, model_config)) = provider
        .models
        .iter()
        .find(|(id, _)| *id == original.model_id)
    else {
        return Effective::none("model-not-found");
    };
    let mut normalized = original.clone();
    let mut validation = validate_options(model_config, &normalized);
    if validation == Validation::ReasoningLevelNotSupported
        && let Some(level) = legacy.and_then(|f| {
            f(&Selection {
                provider_id: provider_id.clone(),
                ..original.clone()
            })
        })
    {
        let mut options = original.options.clone().unwrap_or_default();
        options.insert("reasoningLevel".into(), level.into());
        let candidate = Selection {
            options: Some(options),
            ..original.clone()
        };
        if validate_options(model_config, &candidate) == Validation::Ok {
            normalized = candidate;
            validation = Validation::Ok;
        }
    }
    let result = Selection {
        provider_id,
        model_id: original.model_id.clone(),
        options: if validation == Validation::Ok {
            normalized.options
        } else {
            None
        },
    };
    Effective {
        selection: Some(result),
        issue: match validation {
            Validation::ReasoningLevelMissing => Some("reasoning-level-missing"),
            Validation::ReasoningLevelNotSupported => Some("reasoning-level-not-supported"),
            Validation::Ok => None,
        },
    }
}

/// TS `completeNewModelSelection`：最高推理档。
fn complete_new(model_config: &Value, provider_id: &str, model_id: &str) -> Option<Selection> {
    let level = levels(model_config).last().copied()?;
    let mut options = Map::new();
    options.insert("reasoningLevel".into(), level.into());
    Some(Selection {
        provider_id: provider_id.into(),
        model_id: model_id.into(),
        options: Some(options),
    })
}

fn is_selectable(registry: &[RegistryProvider], selection: &Selection) -> bool {
    let Some(provider) = registry
        .iter()
        .find(|p| p.provider_id == selection.provider_id)
    else {
        return false;
    };
    if provider.config.get("visibility").and_then(Value::as_str) == Some("hidden") {
        return false;
    }
    provider
        .models
        .iter()
        .find(|(id, _)| *id == selection.model_id)
        .is_some_and(|(_, config)| validate_options(config, selection) == Validation::Ok)
}

/// TS `resolveInitialModelSelection`。
pub fn resolve_initial(
    registry: &[RegistryProvider],
    configured_default: Option<&Selection>,
) -> Option<Selection> {
    if let Some(default) = configured_default
        && is_selectable(registry, default)
    {
        return Some(default.clone());
    }
    for provider in registry {
        if provider.config.get("visibility").and_then(Value::as_str) == Some("hidden") {
            continue;
        }
        for (model_id, config) in &provider.models {
            if let Some(selection) = complete_new(config, &provider.provider_id, model_id) {
                return Some(selection);
            }
        }
    }
    None
}

/// TS `serializeRegistryProviderConfig`。
fn serialize_provider(config: &Value) -> Value {
    let mut out = Map::new();
    let get = |key: &str| config.get(key);
    if let Some(group) = get("group") {
        out.insert("group".into(), group.clone());
    }
    if let Some(logo) = get("logo") {
        out.insert("logo".into(), logo.clone());
    }
    let access = &config["access"];
    let mut access_out = Map::new();
    access_out.insert("type".into(), access["type"].clone());
    if access["type"] == "zhipu-account" {
        for key in ["accountType", "mode", "entitled"] {
            access_out.insert(key.into(), access[key].clone());
        }
    } else {
        access_out.insert("apiKey".into(), access["apiKey"].clone());
        if let Some(url) = access.get("apiKeyManagementUrl") {
            access_out.insert("apiKeyManagementUrl".into(), url.clone());
        }
    }
    out.insert("access".into(), Value::Object(access_out));
    let api = &config["api"];
    let mut api_out = Map::new();
    api_out.insert("type".into(), api["type"].clone());
    api_out.insert("baseUrl".into(), api["baseUrl"].clone());
    if let Some(headers) = api.get("headers").filter(|h| !h.is_null()) {
        api_out.insert("headers".into(), headers.clone());
    }
    out.insert("api".into(), Value::Object(api_out));
    for key in ["builtinModelIds", "personalModelIds", "modelOrder"] {
        if let Some(ids) = get(key).filter(|v| !v.is_null()) {
            out.insert(key.into(), ids.clone());
        }
    }
    if let Some(visibility) = get("visibility") {
        out.insert("visibility".into(), visibility.clone());
    }
    Value::Object(out)
}

/// TS `serializeRegistryModelConfig`。
fn serialize_model(config: &Value) -> Value {
    let p = &config["properties"];
    json!({
        "enabled": config["enabled"],
        "properties": {
            "requiresMfjsToolSchema": p["requiresMfjsToolSchema"],
            "contextWindow": p["contextWindow"],
            "inputFormat": {
                "supportsText": p["inputFormat"]["supportsText"],
                "supportsImage": p["inputFormat"]["supportsImage"],
                "supportsVideo": p["inputFormat"]["supportsVideo"],
                "supportsAudio": p["inputFormat"]["supportsAudio"],
                "supportsPdf": p["inputFormat"]["supportsPdf"],
            },
            "outputFormat": {"supportsText": p["outputFormat"]["supportsText"]},
            "supportsToolCall": p["supportsToolCall"],
            "supportsJsonSchemaOutput": p["supportsJsonSchemaOutput"],
            "supportsNativeWebSearch": p["supportsNativeWebSearch"],
            "supportsMidConversationSystem": p["supportsMidConversationSystem"],
        },
        "optionSpecs": {
            "reasoningLevel": config["optionSpecs"]["reasoningLevel"],
            "maxOutputTokens": config["optionSpecs"]["maxOutputTokens"],
        },
    })
}

/// TS `projectModelSelectionProviderView`。
pub fn project_provider(provider: &RegistryProvider) -> Value {
    let mut object = Map::new();
    object.insert("providerId".into(), provider.provider_id.clone().into());
    if let Some(name) = &provider.provider_name {
        object.insert("providerName".into(), name.clone());
    }
    if let Some(template) = &provider.template_id {
        object.insert("templateId".into(), template.clone().into());
    }
    object.insert("config".into(), serialize_provider(&provider.config));
    object.insert(
        "models".into(),
        Value::Array(
            provider
                .models
                .iter()
                .map(|(id, config)| json!({"modelId": id, "config": serialize_model(config)}))
                .collect(),
        ),
    );
    Value::Object(object)
}
