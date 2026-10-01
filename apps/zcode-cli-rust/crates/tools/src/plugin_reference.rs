//! `plugins/referenceCatalog(WithCategory)`（docs/specs/rust-plugins.md 第 3 期）：对话 Picker 的插件
//! 引用目录，对齐 TS `zcode-protocol/plugin-reference-catalog.ts` + core `buildPluginReferenceCatalog`。
//!
//! 身份部分（id / enabled / 冲突 / skill 与子代理限定名 / MCP 名）在会话里冻结（由 engine 持有）；
//! 展示部分（icon / displayName / description / category）每次从市场 listing 现取——与 TS 一样，
//! listing 是可变展示投影，不属于冻结的会话身份。

use super::{extension_config as config, extension_plugins as plugins, plugin_list};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

/// 返回 `{identity, plugins}`：`identity` 是本次使用的身份条目（`frozen` 为空时现算，供 engine 冻结），
/// `plugins` 是协议条目。
pub(super) async fn catalog(
    params: &Value,
    frozen: Option<&Value>,
    include_category: bool,
    cancel: &CancellationToken,
) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let identity = match frozen {
        Some(identity) => identity.clone(),
        None => identity_entries(&cwd, cancel).await?,
    };
    let display = display_by_plugin_id(params, cancel).await?;
    let entries = identity
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|entry| to_entry(entry, &display, include_category))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(json!({ "identity": identity, "plugins": entries }))
}

/// core `buildPluginReferenceCatalog`：全部已发现插件（含停用）；同 manifest name 的多个**启用**插件互标冲突，
/// 停用条目不参与冲突。
pub(super) async fn identity_entries(cwd: &std::path::Path, cancel: &CancellationToken) -> Result<Value> {
    let config = config::load(cwd).await?;
    let data_root = config::storage(&config).join("data");
    let discovered = plugins::all(cwd, &config, cancel).await?;
    let mut entries = vec![];
    for plugin in &discovered {
        let info = plugin_list::info(plugin, cwd, &config, &data_root).await?;
        let mut conflicts: Vec<&str> = if plugin.enabled {
            discovered
                .iter()
                .filter(|other| other.enabled && other.name == plugin.name && other.id != plugin.id)
                .map(|other| other.id.as_str())
                .collect()
        } else {
            vec![]
        };
        conflicts.sort();
        let mut mcp: Vec<String> = info["mcpServerNames"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        mcp.sort();
        entries.push(json!({
            "pluginId": plugin.id,
            "name": plugin.name,
            "marketplace": plugin.marketplace,
            "enabled": plugin.enabled,
            "conflictingPluginIds": conflicts,
            "skillQualifiedNames": qualified_names(&info, &plugin.name, "skill"),
            "mcpServerNames": mcp,
            "subagentNames": qualified_names(&info, &plugin.name, "agent"),
        }));
    }
    Ok(Value::Array(entries))
}

/// core `collectDeclared*Names`：`<plugin name>:<component name>`，去重排序。
fn qualified_names(info: &Value, plugin_name: &str, kind: &str) -> Vec<String> {
    let mut names: Vec<String> = info["components"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|group| group["kind"] == kind)
        .flat_map(|group| group["items"].as_array().cloned().unwrap_or_default())
        .filter_map(|item| item["name"].as_str().map(str::trim).map(str::to_owned))
        .filter(|name| !name.is_empty())
        .map(|name| format!("{plugin_name}:{name}"))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// TS `resolveReferenceListingDisplayByPluginId`：available → installed → restorable 依次合并，
/// 后者的非空字段覆盖前者。
pub(super) async fn display_by_plugin_id(
    params: &Value,
    cancel: &CancellationToken,
) -> Result<HashMap<String, Map<String, Value>>> {
    let overview =
        super::plugin_overview::overview(&json!({ "workspace": params["workspace"] }), cancel)
            .await?;
    let mut display: HashMap<String, Map<String, Value>> = HashMap::new();
    for key in ["availablePlugins", "installedPlugins", "restorableBuiltins"] {
        for plugin in overview[key].as_array().into_iter().flatten() {
            let listing = &plugin["listing"];
            let trimmed = |value: &Value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::from(s.to_owned()))
            };
            let mut fields = Map::new();
            for (field, value) in [
                ("category", trimmed(&listing["category"])),
                ("icon", trimmed(&listing["icon"])),
                ("displayName", trimmed(&listing["displayName"])),
                (
                    "displayNameI18n",
                    listing["displayNameI18n"]
                        .is_object()
                        .then(|| listing["displayNameI18n"].clone()),
                ),
                ("description", trimmed(&plugin["description"])),
                (
                    "descriptionI18n",
                    listing["descriptionI18n"]
                        .is_object()
                        .then(|| listing["descriptionI18n"].clone()),
                ),
            ] {
                if let Some(value) = value {
                    fields.insert(field.into(), value);
                }
            }
            if fields.is_empty() {
                continue;
            }
            let Some(id) = plugin["id"].as_str() else {
                continue;
            };
            display.entry(id.to_owned()).or_default().extend(fields);
        }
    }
    Ok(display)
}

/// TS `toReferenceCatalogEntry`：身份字段 + 展示字段；`rootPath` 不出协议。
fn to_entry(
    entry: &Value,
    display: &HashMap<String, Map<String, Value>>,
    include_category: bool,
) -> Value {
    let id = entry["pluginId"].as_str().unwrap_or_default();
    let shown = display.get(id);
    let mut out = Map::new();
    if include_category {
        out.insert(
            "category".into(),
            shown
                .and_then(|d| d.get("category").cloned())
                .unwrap_or_else(|| "other".into()),
        );
    }
    for key in ["pluginId", "name", "marketplace"] {
        out.insert(key.into(), entry[key].clone());
    }
    for key in [
        "icon",
        "displayName",
        "displayNameI18n",
        "description",
        "descriptionI18n",
    ] {
        if let Some(value) = shown.and_then(|d| d.get(key)) {
            out.insert(key.into(), value.clone());
        }
    }
    for key in [
        "enabled",
        "conflictingPluginIds",
        "skillQualifiedNames",
        "mcpServerNames",
        "subagentNames",
    ] {
        out.insert(key.into(), entry[key].clone());
    }
    Value::Object(out)
}
