//! Personal 配置写入面（TS `ProviderConfigService` 的 Provider 级操作）。
//!
//! 逐条对照 `packages/provider/src/config-service.ts`；错误文案原样保留。Model 级操作见
//! [`super::mutations_models`]。所有操作都在 [`super::store::update`] 的文件锁内对最新文档执行。

use serde_json::{Map, Value, json};

use super::personal::Doc;
use super::resolve::{ProviderMap, Template, resolve_owned_order};
use super::sparse;
use super::store::MutationError;
use crate::git::paths::js_trim;

pub type Result<T> = std::result::Result<T, MutationError>;

/// Built-in 事实（只读）。
pub struct Builtin<'a> {
    pub providers: &'a ProviderMap,
    pub templates: &'a [Template],
}

/// TS `ProviderModelMembership`：Host 内部算出的继承成员名单与发布快照的 Personal 版本。
pub struct Membership {
    pub provider_id: String,
    pub inherited: Vec<String>,
    pub personal_revision: String,
}

pub fn assert_membership_current(
    membership: &Membership,
    provider_id: &str,
    doc: &Doc,
) -> Result<()> {
    if membership.provider_id != provider_id || membership.personal_revision != doc.revision() {
        return Err("Provider Settings membership revision conflict".into());
    }
    Ok(())
}

pub fn assert_non_empty_id(label: &str, value: &str) -> Result<()> {
    if js_trim(value).is_empty() {
        return Err(format!("{label} 不能为空").into());
    }
    Ok(())
}

pub fn normalize_id(label: &str, value: &str) -> Result<String> {
    let normalized = js_trim(value);
    if normalized.is_empty() {
        return Err(format!("{label} 不能为空").into());
    }
    Ok(normalized.to_owned())
}

/// 配置里的 ID 列表；缺省与 null 同为空（TS `?? []`）。
pub fn ids(config: &Value, key: &str) -> Vec<String> {
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

pub fn with_key(config: &Value, key: &str, value: Value) -> Value {
    let mut map = config.as_object().cloned().unwrap_or_default();
    map.insert(key.to_owned(), value);
    Value::Object(map)
}

pub fn unique_in_order(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .iter()
        .filter(|v| seen.insert((*v).clone()))
        .cloned()
        .collect()
}

pub fn normalize_model_order(
    builtin: &[String],
    personal: &[String],
    requested: &[String],
) -> Vec<String> {
    resolve_owned_order(builtin, personal, requested)
}

pub(super) fn normalize_provider_order(
    providers: &ProviderMap,
    requested: &[String],
) -> Vec<String> {
    let personal: Vec<String> = providers
        .0
        .iter()
        .filter(|rule| {
            rule.config.get("group").and_then(Value::as_str) == Some("standard-personal")
        })
        .map(|rule| rule.provider_id.clone())
        .collect();
    resolve_owned_order(&[], &personal, requested)
}

/// TS `writableProviderOverlay`：Personal 根记录只是覆盖层；仅继承 Provider 可按需创建覆盖。
pub fn writable_overlay(builtin: &Builtin<'_>, doc: &Doc, provider_id: &str) -> Result<Value> {
    if let Some(rule) = doc.providers.get(provider_id) {
        return Ok(rule.config.clone());
    }
    if builtin.providers.has(provider_id) {
        return Ok(json!({}));
    }
    Err(format!("Provider 不存在: {provider_id}").into())
}

fn label_of(providers: &ProviderMap, provider_id: &str) -> Option<String> {
    let name = providers
        .get(provider_id)?
        .provider_name
        .as_ref()?
        .as_str()?;
    Some(js_trim(name).to_owned())
}

/// 本次改名不得新引入重名（历史重名不阻断无关 Provider 的保存）。
fn assert_label_unique(provider_id: &str, current: &ProviderMap, next: &ProviderMap) -> Result<()> {
    let current_key = label_of(current, provider_id).map(|label| label.to_lowercase());
    let next_label = label_of(next, provider_id);
    let next_key = next_label.as_ref().map(|label| label.to_lowercase());
    let Some(next_key) = next_key.filter(|key| !key.is_empty()) else {
        return Ok(());
    };
    if Some(&next_key) == current_key.as_ref() {
        return Ok(());
    }
    for candidate in &next.0 {
        if candidate.provider_id == provider_id {
            continue;
        }
        let name = candidate.provider_name.as_ref().and_then(Value::as_str);
        if name.map(|name| js_trim(name).to_lowercase()) == Some(next_key.clone()) {
            return Err(format!("Provider 名称已存在: {}", next_label.unwrap_or_default()).into());
        }
    }
    Ok(())
}

/// TS `normalizePersonalProviderMembership`：成员去重、剔除与继承名单重叠者、顺序归一。
fn normalized_membership(personal: Option<&Value>, inherited: &[String]) -> Option<Value> {
    let personal = personal?;
    let builtin = unique_in_order(inherited);
    let own: Vec<String> = unique_in_order(&ids(personal, "personalModelIds"))
        .into_iter()
        .filter(|id| !builtin.contains(id))
        .collect();
    let mut normalized = with_key(personal, "personalModelIds", json!(own));
    if personal
        .get("modelOrder")
        .is_some_and(|order| !order.is_null())
    {
        let order = normalize_model_order(&builtin, &own, &ids(personal, "modelOrder"));
        normalized = with_key(&normalized, "modelOrder", json!(order));
    }
    Some(normalized)
}

/// TS `withModelMembershipFrom`：展示/连接字段取自 `config`，成员字段只取自 `source`。
fn with_membership_from(config: &Value, source: Option<&Value>) -> Value {
    let mut map = Map::new();
    for key in ["group", "logo", "access", "api", "visibility"] {
        if let Some(value) = config.get(key) {
            map.insert(key.to_owned(), value.clone());
        }
    }
    for key in ["builtinModelIds", "personalModelIds", "modelOrder"] {
        if let Some(value) = source.and_then(|source| source.get(key)) {
            map.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(map)
}

/// `savePersonalProviderOverlay` 的 metadata（`None` = TS `undefined`）。
#[derive(Default)]
pub struct Metadata {
    pub provider_name: Option<Value>,
    pub template_id: Option<Value>,
    pub enabled: Option<Value>,
}

pub fn save_provider_overlay(
    doc: &mut Doc,
    builtin: &Builtin<'_>,
    provider_id: &str,
    config: &Value,
    membership: &Membership,
    metadata: &Metadata,
) -> Result<()> {
    assert_non_empty_id("providerId", provider_id)?;
    sparse::provider_config(config)?;
    assert_membership_current(membership, provider_id, doc)?;
    let builtin_rule = builtin.providers.get(provider_id);
    let current_personal = doc
        .providers
        .get(provider_id)
        .map(|rule| rule.config.clone());
    let current_effective = builtin.providers.overlay(&doc.providers);
    let is_account = builtin_rule
        .and_then(|rule| rule.config.get("access"))
        .and_then(|access| access.get("type"))
        .and_then(Value::as_str)
        == Some("zhipu-account");
    if is_account && metadata.enabled == Some(Value::Bool(false)) {
        return Err(format!("Account Provider 不允许禁用: {provider_id}").into());
    }
    if is_account && config.get("access").is_some() {
        return Err(format!(
            "固定 Account Provider 的 Access 只能由 ESCode Built-in Config 声明: {provider_id}"
        )
        .into());
    }
    if current_personal.is_none() && builtin_rule.is_none() {
        return Err(format!("Personal Provider 尚未创建: {provider_id}").into());
    }
    let group_of = |value: &Value| {
        value
            .get("group")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let normalized = if let Some(rule) = builtin_rule {
        let requested = group_of(config);
        if requested.is_some() && requested != group_of(&rule.config) {
            return Err(format!(
                "Personal Overlay 不能改写 Built-in Provider group: {provider_id}"
            )
            .into());
        }
        super::overlay::without_group(config)
    } else {
        let group = group_of(config).or_else(|| current_personal.as_ref().and_then(group_of));
        if group.as_deref() != Some("standard-personal") {
            return Err(format!(
                "Personal-only Provider 必须使用 standard-personal group: {provider_id}"
            )
            .into());
        }
        with_key(config, "group", json!("standard-personal"))
    };
    let next = with_membership_from(
        &normalized,
        normalized_membership(current_personal.as_ref(), &membership.inherited).as_ref(),
    );
    let mut rule = doc.providers.get(provider_id).cloned().unwrap_or_default();
    rule.provider_id = provider_id.to_owned();
    if let Some(template_id) = &metadata.template_id {
        rule.template_id = Some(template_id.clone());
    }
    if let Some(enabled) = &metadata.enabled {
        rule.enabled = Some(enabled.clone());
    }
    if let Some(name) = &metadata.provider_name {
        // TS：`providerName?.trim() || null`
        let trimmed = name.as_str().map(js_trim).filter(|name| !name.is_empty());
        rule.provider_name = Some(trimmed.map_or(Value::Null, |name| json!(name)));
    }
    rule.config = next;
    let mut providers = doc.providers.clone();
    providers.set_rule(rule);
    assert_label_unique(
        provider_id,
        &current_effective,
        &builtin.providers.overlay(&providers),
    )?;
    doc.providers = providers;
    Ok(())
}

pub fn delete_provider(doc: &mut Doc, provider_id: &str) -> Result<()> {
    assert_non_empty_id("providerId", provider_id)?;
    doc.providers.delete(provider_id);
    doc.delete_exact_for_provider(provider_id);
    if let Some(order) = &mut doc.provider_order {
        order.retain(|candidate| candidate != provider_id);
    }
    Ok(())
}

pub fn reorder_providers(doc: &mut Doc, requested: &[String]) {
    doc.provider_order = Some(normalize_provider_order(&doc.providers, requested));
}

pub fn reorder_models(
    doc: &mut Doc,
    builtin: &Builtin<'_>,
    provider_id: &str,
    requested: &[String],
    membership: &Membership,
) -> Result<()> {
    assert_non_empty_id("providerId", provider_id)?;
    assert_membership_current(membership, provider_id, doc)?;
    let provider = writable_overlay(builtin, doc, provider_id)?;
    let order = normalize_model_order(
        &membership.inherited,
        &ids(&provider, "personalModelIds"),
        requested,
    );
    doc.providers
        .set_config(provider_id, with_key(&provider, "modelOrder", json!(order)));
    Ok(())
}
