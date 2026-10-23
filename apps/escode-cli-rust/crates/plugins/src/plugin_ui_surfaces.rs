//! `plugins/listUiSurfaces`：已启用插件清单 `ui.surfaces[]` 的只读投影。
//!
//! 对齐 TS `adapters/src/plugins/ui-surfaces.ts::parsePluginUiSurfaces` 与
//! `escode-protocol/plugin-ui-surfaces.ts`：非法项丢弃，不影响其余面板。
use anyhow::Result;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

const TITLE_MAX_CHARS: usize = 200;
const ICON_MAX_CHARS: usize = 2048;
const ID_MAX_CHARS: usize = 128;
const URI_MAX_CHARS: usize = 2048;

pub async fn list(params: &Value) -> Result<Value> {
    let cwd = super::plugin_list::workspace_path(params)?;
    let layers = super::plugin_list::layers_for(&cwd, params).await?;
    let config = layers.merged();
    let (discovered, _) =
        super::extension_plugins::discover(&cwd, &config, &CancellationToken::new()).await?;
    let mut surfaces = Vec::new();
    for plugin in discovered.iter().filter(|plugin| plugin.enabled) {
        let declared = declared_servers(&plugin.manifest);
        for raw in plugin.manifest["ui"]["surfaces"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(surface) = parse_surface(raw, &declared, &plugin.name) {
                if surfaces.iter().any(|item: &Value| {
                    item["pluginId"] == plugin.id.as_str() && item["id"] == surface["id"]
                }) {
                    continue;
                }
                let mut entry = Map::new();
                entry.insert("pluginId".into(), json!(plugin.id));
                entry.insert("pluginName".into(), json!(plugin.name));
                for (key, value) in surface {
                    entry.insert(key, value);
                }
                surfaces.push(Value::Object(entry));
            }
        }
    }
    Ok(json!({ "surfaces": surfaces }))
}

/// 清单 `mcpServers` 的声明键（面板 `server` 必须指向其中之一）。
fn declared_servers(manifest: &Value) -> Vec<String> {
    manifest["mcpServers"]
        .as_object()
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
}

/// 单个面板项；返回 `id/title/icon/server/resourceUri/availability`。
fn parse_surface(
    raw: &Value,
    declared: &[String],
    plugin_name: &str,
) -> Option<Map<String, Value>> {
    let id = read_id(&raw["id"])?;
    let title = read_title(&raw["title"])?;
    let declared_server = raw["server"].as_str().map(str::trim).unwrap_or_default();
    if declared_server.is_empty() || !declared.iter().any(|name| name == declared_server) {
        return None;
    }
    let resource_uri = raw["resourceUri"]
        .as_str()
        .map(str::trim)
        .unwrap_or_default();
    if !resource_uri.starts_with("ui://") || resource_uri.len() > URI_MAX_CHARS {
        return None;
    }
    if raw["availability"] != "session" {
        return None;
    }
    let mut surface = Map::new();
    surface.insert("id".into(), json!(id));
    surface.insert("title".into(), title);
    if let Some(icon) = raw["icon"]
        .as_str()
        .map(str::trim)
        .filter(|icon| !icon.is_empty())
    {
        surface.insert("icon".into(), json!(truncate(icon, ICON_MAX_CHARS)));
    }
    surface.insert(
        "server".into(),
        json!(format!("plugin:{plugin_name}:{declared_server}")),
    );
    surface.insert("resourceUri".into(), json!(resource_uri));
    surface.insert("availability".into(), json!("session"));
    Some(surface)
}

/// `[A-Za-z0-9][A-Za-z0-9_.-]*`，最长 128。
fn read_id(value: &Value) -> Option<String> {
    let id = value.as_str()?.trim();
    if id.is_empty() || id.len() > ID_MAX_CHARS {
        return None;
    }
    let mut chars = id.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    chars
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
        .then(|| id.to_owned())
}

/// 非空字符串，或 locale → 非空字符串 的映射（丢弃空项）。
fn read_title(value: &Value) -> Option<Value> {
    if let Some(title) = value
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        return Some(json!(truncate(title, TITLE_MAX_CHARS)));
    }
    let record = value.as_object()?;
    let mut titles = Map::new();
    for (locale, text) in record {
        if let Some(text) = text.as_str().map(str::trim).filter(|text| !text.is_empty()) {
            titles.insert(locale.clone(), json!(truncate(text, TITLE_MAX_CHARS)));
        }
    }
    (!titles.is_empty()).then_some(Value::Object(titles))
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
#[path = "plugin_ui_surfaces_tests.rs"]
mod tests;
