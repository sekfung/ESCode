//! Personal 配置写入面：创建 Provider（TS `ProviderConfigService.createPersonalProvider`）。

use serde_json::{Map, Value, json};

use super::mutations::{Builtin, Result, normalize_provider_order};
use super::personal::Doc;
use super::resolve::{ProviderMap, ProviderRule, Template};
use super::sparse;
use crate::git::paths::js_trim;

/// 创建入参（Facade 只在值真值时传入各字段）。
#[derive(Default)]
pub struct CreateInput {
    pub template_id: Option<String>,
    pub provider_name: Option<String>,
    pub locale: Option<String>,
    pub initial_config: Option<Value>,
}

fn provider_id_seed(value: &str) -> String {
    let lowered = js_trim(value).to_lowercase();
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "new-provider".to_owned()
    } else {
        out
    }
}

fn next_provider_id(occupied: &[String], template_id: Option<&str>) -> String {
    let base = template_id.map_or_else(|| "new-provider".to_owned(), provider_id_seed);
    if !occupied.contains(&base) {
        return base;
    }
    (2..)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|candidate| !occupied.contains(candidate))
        .unwrap_or(base)
}

fn next_label(seed: &str, providers: &ProviderMap) -> String {
    let base = match js_trim(seed) {
        "" => "new-provider",
        trimmed => trimmed,
    };
    let labels: std::collections::HashSet<String> = providers
        .0
        .iter()
        .filter_map(|rule| rule.provider_name.as_ref()?.as_str())
        .map(|name| js_trim(name).to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    if !labels.contains(&base.to_lowercase()) {
        return base.to_owned();
    }
    (2..)
        .map(|suffix| format!("{base} {suffix}"))
        .find(|candidate| !labels.contains(&candidate.to_lowercase()))
        .unwrap_or_else(|| base.to_owned())
}

/// TS `resolveProviderTemplateName`。
fn template_name(template: &Template, locale: &str) -> String {
    let pick = |key: &str| {
        template
            .name_map
            .get(key)
            .and_then(Value::as_str)
            .map(|name| js_trim(name).to_owned())
            .filter(|name| !name.is_empty())
    };
    pick(locale)
        .or_else(|| pick("en-US"))
        .unwrap_or_else(|| template.template_id.clone())
}

pub fn create_provider(
    doc: &mut Doc,
    builtin: &Builtin<'_>,
    input: &CreateInput,
) -> Result<String> {
    let template_id = input
        .template_id
        .as_deref()
        .map(js_trim)
        .filter(|id| !id.is_empty());
    let template = match template_id {
        Some(id) => Some(
            builtin
                .templates
                .iter()
                .find(|template| template.template_id == id)
                .ok_or_else(|| format!("Provider Template 不存在: {id}"))?,
        ),
        None => None,
    };
    if let Some(initial) = &input.initial_config {
        sparse::provider_config(initial)?;
        if initial.get("group").is_some() {
            return Err("initialConfig 不能包含 group".into());
        }
        if initial.get("builtinModelIds").is_some() {
            return Err("initialConfig 不能包含 builtinModelIds".into());
        }
    }
    let mut occupied = builtin.providers.keys();
    occupied.extend(doc.providers.keys());
    let provider_id = next_provider_id(&occupied, template_id);
    let effective = builtin.providers.overlay(&doc.providers);
    let seed = match (&input.provider_name, template) {
        (Some(name), _) => name.clone(),
        (None, Some(template)) => {
            template_name(template, input.locale.as_deref().unwrap_or("en-US"))
        }
        (None, None) => "new-provider".to_owned(),
    };
    let label = next_label(&seed, &effective);
    let mut base = Map::new();
    base.insert("group".into(), json!("standard-personal"));
    if template_id.is_none() {
        base.insert("access".into(), json!({"type": "api-key"}));
    }
    base.insert("personalModelIds".into(), json!([]));
    base.insert("modelOrder".into(), json!([]));
    let initial = input.initial_config.clone().unwrap_or_else(|| json!({}));
    let config = super::overlay::overlay(
        super::overlay::Kind::Provider,
        &Value::Object(base),
        &initial,
    );
    doc.providers.set_rule(ProviderRule {
        provider_id: provider_id.clone(),
        template_id: template_id.map(|id| json!(id)),
        provider_name: Some(json!(label)),
        enabled: None,
        config,
    });
    let current =
        normalize_provider_order(&doc.providers, doc.provider_order.as_deref().unwrap_or(&[]));
    let mut requested: Vec<String> = current
        .into_iter()
        .filter(|id| *id != provider_id)
        .collect();
    requested.push(provider_id.clone());
    doc.provider_order = Some(normalize_provider_order(&doc.providers, &requested));
    Ok(provider_id)
}
