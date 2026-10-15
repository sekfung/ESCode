//! Personal 配置写入面：Model 级操作（TS `ProviderConfigService` 的 add/rename/delete/enable/draft）。

use serde_json::{Value, json};

use super::mutations::{
    Builtin, Membership, Result, assert_membership_current, ids, normalize_id,
    normalize_model_order, with_key, writable_overlay,
};
use super::overlay::{Kind, overlay};
use super::personal::Doc;
use super::sparse;
use super::store::Write;

fn exists_msg(provider_id: &str, model_id: &str) -> String {
    format!("Model 已存在: {provider_id}/{model_id}")
}

/// 把成员名单与顺序写回 Provider 的 Personal 覆盖。
fn put_members(
    doc: &mut Doc,
    provider_id: &str,
    provider: &Value,
    builtin: &[String],
    members: &[String],
    order: &[String],
) {
    let config = with_key(provider, "personalModelIds", json!(members));
    let config = with_key(
        &config,
        "modelOrder",
        json!(normalize_model_order(builtin, members, order)),
    );
    doc.providers.set_config(provider_id, config);
}

pub fn add_model(
    doc: &mut Doc,
    builtin: &Builtin<'_>,
    provider_id: &str,
    model_id: &str,
    config: &Value,
    membership: &Membership,
    use_recommended: Option<bool>,
) -> Result<()> {
    let provider_id = normalize_id("providerId", provider_id)?;
    let model_id = normalize_id("modelId", model_id)?;
    sparse::model_config(config)?;
    assert_membership_current(membership, &provider_id, doc)?;
    let provider = writable_overlay(builtin, doc, &provider_id)?;
    if membership.inherited.contains(&model_id) {
        return Err(exists_msg(&provider_id, &model_id).into());
    }
    let current = ids(&provider, "personalModelIds");
    if current.contains(&model_id) {
        return Err(exists_msg(&provider_id, &model_id).into());
    }
    let mut members = current;
    members.push(model_id.clone());
    // 添加不能重新按成员名单排序，否则会丢掉用户已经保存的顺序。
    let order = ids(&provider, "modelOrder");
    put_members(
        doc,
        &provider_id,
        &provider,
        &membership.inherited,
        &members,
        &order,
    );
    let config = overlay(Kind::Model, config, &json!({"enabled": true}));
    doc.set_exact(&provider_id, &model_id, config, use_recommended)?;
    Ok(())
}

/// 成员改名：成员名单、顺序与精确规则一并迁移。
fn rename_member(
    doc: &mut Doc,
    provider_id: &str,
    provider: &Value,
    inherited: &[String],
    current: &str,
    next: &str,
) {
    let rename = |list: Vec<String>| -> Vec<String> {
        list.into_iter()
            .map(|id| if id == current { next.to_owned() } else { id })
            .collect()
    };
    let members = rename(ids(provider, "personalModelIds"));
    let order = rename(ids(provider, "modelOrder"));
    put_members(doc, provider_id, provider, inherited, &members, &order);
    doc.rename_exact_model(provider_id, current, next);
}

pub fn rename_model(
    doc: &mut Doc,
    provider_id: &str,
    current_id: &str,
    next_id: &str,
    membership: &Membership,
) -> Result<Write> {
    let provider_id = normalize_id("providerId", provider_id)?;
    let current_id = normalize_id("modelId", current_id)?;
    let next_id = normalize_id("modelId", next_id)?;
    if current_id == next_id {
        return Ok(Write::Skip);
    }
    assert_membership_current(membership, &provider_id, doc)?;
    let provider = doc
        .providers
        .get(&provider_id)
        .map(|rule| rule.config.clone());
    if membership.inherited.contains(&current_id) {
        return Err(format!("Built-in Model 不能重命名: {provider_id}/{current_id}").into());
    }
    let Some(provider) = provider.filter(|p| ids(p, "personalModelIds").contains(&current_id))
    else {
        return Err(format!("Personal Model 不存在: {provider_id}/{current_id}").into());
    };
    if ids(&provider, "personalModelIds").contains(&next_id)
        || membership.inherited.contains(&next_id)
    {
        return Err(exists_msg(&provider_id, &next_id).into());
    }
    rename_member(
        doc,
        &provider_id,
        &provider,
        &membership.inherited,
        &current_id,
        &next_id,
    );
    Ok(Write::Commit)
}

pub fn set_model_enabled(
    doc: &mut Doc,
    provider_id: &str,
    model_id: &str,
    enabled: &Value,
    membership: &Membership,
) -> Result<()> {
    let provider_id = normalize_id("providerId", provider_id)?;
    let model_id = normalize_id("modelId", model_id)?;
    let Some(enabled) = enabled.as_bool() else {
        return Err("Model enabled 必须是 boolean".into());
    };
    assert_membership_current(membership, &provider_id, doc)?;
    let owned = doc
        .providers
        .get(&provider_id)
        .is_some_and(|rule| ids(&rule.config, "personalModelIds").contains(&model_id));
    if !membership.inherited.contains(&model_id) && !owned {
        return Err(format!("Model 不存在: {provider_id}/{model_id}").into());
    }
    // 启停只改最新 enabled；不改变模式、成员和其他模型字段。
    let current = doc
        .exact_config(&provider_id, &model_id)
        .unwrap_or_else(|| json!({}));
    let config = overlay(Kind::Model, &current, &json!({"enabled": enabled}));
    doc.set_exact(&provider_id, &model_id, config, None)?;
    Ok(())
}

pub fn delete_model(
    doc: &mut Doc,
    provider_id: &str,
    model_id: &str,
    membership: &Membership,
) -> Result<()> {
    let provider_id = normalize_id("providerId", provider_id)?;
    let model_id = normalize_id("modelId", model_id)?;
    assert_membership_current(membership, &provider_id, doc)?;
    let provider = doc
        .providers
        .get(&provider_id)
        .map(|rule| rule.config.clone());
    if membership.inherited.contains(&model_id) {
        return Err(format!("Built-in Model 不能删除: {provider_id}/{model_id}").into());
    }
    let Some(provider) = provider.filter(|p| ids(p, "personalModelIds").contains(&model_id)) else {
        return Err(format!("Personal Model 不存在: {provider_id}/{model_id}").into());
    };
    let members: Vec<String> = ids(&provider, "personalModelIds")
        .into_iter()
        .filter(|id| *id != model_id)
        .collect();
    let order = ids(&provider, "modelOrder");
    put_members(
        doc,
        &provider_id,
        &provider,
        &membership.inherited,
        &members,
        &order,
    );
    doc.delete_exact(&provider_id, &model_id);
    Ok(())
}

/// TS `isStructurallyEmpty`。
fn structurally_empty(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.values().all(structurally_empty),
        _ => false,
    }
}

/// Model 编辑弹窗的唯一写入边界：成员、顺序、精确规则在同一次更新里提交。
pub struct DraftInput<'a> {
    pub provider_id: &'a str,
    pub original_id: &'a str,
    pub next_id: &'a str,
    pub config: &'a Value,
    pub expected_revision: &'a str,
    pub use_recommended: Option<bool>,
}

pub fn save_model_draft(
    doc: &mut Doc,
    input: &DraftInput<'_>,
    membership: &Membership,
) -> Result<()> {
    let provider_id = normalize_id("providerId", input.provider_id)?;
    let original = normalize_id("modelId", input.original_id)?;
    let next = normalize_id("modelId", input.next_id)?;
    sparse::model_config(input.config)?;
    assert_membership_current(membership, &provider_id, doc)?;
    let current_revision = doc.revision();
    if current_revision != input.expected_revision {
        return Err(format!(
            "Personal Provider Config revision conflict: expected {}, current {current_revision}",
            input.expected_revision
        )
        .into());
    }
    let recommended = input.use_recommended.unwrap_or_else(|| {
        !doc.exact_rule(&provider_id, &original)
            .is_some_and(|rule| rule.manual)
    });
    let provider = doc
        .providers
        .get(&provider_id)
        .map(|rule| rule.config.clone());
    let inherited = &membership.inherited;
    let renamed = original != next;
    if renamed && inherited.contains(&original) {
        return Err(format!("Built-in Model 不能重命名: {provider_id}/{original}").into());
    }
    if renamed && inherited.contains(&next) {
        return Err(exists_msg(&provider_id, &next).into());
    }
    let own = provider
        .as_ref()
        .map(|p| ids(p, "personalModelIds"))
        .unwrap_or_default();
    if !inherited.contains(&original) && !own.contains(&original) {
        return Err(format!("Model 不存在: {provider_id}/{original}").into());
    }
    if renamed && own.contains(&next) {
        return Err(exists_msg(&provider_id, &next).into());
    }
    if renamed {
        let Some(provider) = provider.filter(|_| own.contains(&original)) else {
            return Err(format!("Personal Model 不存在: {provider_id}/{original}").into());
        };
        rename_member(doc, &provider_id, &provider, inherited, &original, &next);
    }
    if recommended && structurally_empty(input.config) {
        doc.delete_exact(&provider_id, &next);
    } else {
        doc.set_exact(&provider_id, &next, input.config.clone(), Some(recommended))?;
    }
    Ok(())
}
