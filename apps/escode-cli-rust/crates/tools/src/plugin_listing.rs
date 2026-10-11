//! 市场条目的商店展示投影（TS `parseEntryStoreListing` / 作者 / 组件类型 / 源身份 pin）。
#[allow(unused_imports)]
use super::plugin_marketplace::*;

use serde_json::{Map, Value};

/// TS `parseEntryStoreListing`：解析不到有效内容时返回 None。
pub(super) fn listing(entry: &Map<String, Value>) -> Option<Value> {
    let text = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    };
    let string_map = |key: &str| -> Option<Value> {
        let map: Map<String, Value> = entry
            .get(key)?
            .as_object()?
            .iter()
            .filter(|(_, v)| v.is_string())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        (!map.is_empty()).then_some(Value::Object(map))
    };
    let mut listing = Map::new();
    if let Some(v) = text("displayName") {
        listing.insert("displayName".into(), v.into());
    }
    if let Some(v) = string_map("displayName_i18n") {
        listing.insert("displayNameI18n".into(), v);
    }
    if let Some(v) = string_map("description_i18n") {
        listing.insert("descriptionI18n".into(), v);
    }
    for key in [
        "icon",
        "category",
        "homepage",
        "privacyPolicy",
        "termsOfService",
        "heroImage",
    ] {
        if let Some(v) = text(key) {
            listing.insert(key.into(), v.into());
        }
    }
    if let Some((name, url)) = author(entry.get("author").unwrap_or(&Value::Null)) {
        if let Some(name) = name {
            listing.insert("author".into(), name.into());
        }
        if let Some(url) = url {
            listing.insert("authorUrl".into(), url.into());
        }
    }
    let prompts: Vec<Value> = entry
        .get("examplePrompts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !prompts.is_empty() {
        listing.insert("examplePrompts".into(), prompts.into());
    }
    if let Some(map) = entry.get("examplePrompts_i18n").and_then(Value::as_object) {
        let lists: Map<String, Value> = map
            .iter()
            .filter_map(|(locale, list)| {
                let items: Vec<Value> = list
                    .as_array()?
                    .iter()
                    .filter(|v| v.is_string())
                    .cloned()
                    .collect();
                (!items.is_empty()).then(|| (locale.clone(), Value::Array(items)))
            })
            .collect();
        if !lists.is_empty() {
            listing.insert("examplePromptsI18n".into(), Value::Object(lists));
        }
    }
    if entry.get("requiresPaidPlan") == Some(&Value::Bool(true)) {
        listing.insert("requiresPaidPlan".into(), true.into());
    }
    (!listing.is_empty()).then_some(Value::Object(listing))
}

/// TS `normalizeAuthorValue`。
pub(super) fn author(value: &Value) -> Option<(Option<String>, Option<String>)> {
    if let Some(name) = value.as_str() {
        let name = name.trim();
        return (!name.is_empty()).then(|| (Some(name.to_owned()), None));
    }
    let object = value.as_object()?;
    let field = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let (name, url) = (field("name"), field("url"));
    (name.is_some() || url.is_some()).then_some((name, url))
}

/// TS `inferComponentTypes`：按目录条目原样的 key 推断（`in` 语义，值是什么都算）。
pub(super) fn component_types(raw: &Map<String, Value>) -> Vec<Value> {
    [
        ("agents", "agent"),
        ("commands", "command"),
        ("skills", "skill"),
        ("hooks", "hook"),
        ("mcpServers", "mcp"),
        ("lspServers", "lsp"),
    ]
    .into_iter()
    .filter(|(key, _)| raw.contains_key(*key))
    .map(|(_, kind)| kind.into())
    .collect()
}

/// TS `readPluginSourceIdentityPin`：zip url 源的 sha256 > `sha` > 旧写法 `commit`。
pub(super) fn source_pin(source: &Value) -> Option<String> {
    let object = source.as_object()?;
    let is_zip = source["source"] == "url"
        && source["type"] == "zip"
        && source["url"].is_string()
        && source["sha256"].is_string();
    let pick = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    if is_zip && let Some(sha) = pick("sha256").filter(|s| !s.is_empty()) {
        return Some(sha);
    }
    pick("sha").or_else(|| pick("commit"))
}
