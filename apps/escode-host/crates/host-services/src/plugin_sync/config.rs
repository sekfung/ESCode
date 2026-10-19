//! plugin-sync 用户配置读写（TS `pluginSyncService.ts:217-227,404-436,907-938`）。

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::ordered_json::{self, OrderedJson};

pub fn config_path(home: &Path) -> PathBuf {
    home.join(".escode").join("cli").join("config.json")
}

pub fn plugin_root(home: &Path) -> PathBuf {
    home.join(".escode").join("plugins")
}

pub fn marketplace_root(home: &Path) -> PathBuf {
    plugin_root(home).join("marketplace-sources")
}

pub fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    lexical(&joined)
}

pub fn lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub fn read_user_config(home: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(config_path(home)) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw).map_err(|error| error.to_string())? {
            value @ Value::Object(_) => Ok(value),
            _ => Err(format!(
                "invalid json object: {}",
                config_path(home).display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.to_string()),
    }
}

pub fn add_plugin_dir(
    home: &Path,
    plugin_path: &Path,
    plugin_id: &str,
    enabled: Option<bool>,
) -> Result<(), String> {
    let path = config_path(home);
    let existing = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| ordered_json::parse_object(&text))
        .unwrap_or_default();
    let mut root = existing;
    let mut plugins = root
        .get("plugins")
        .and_then(OrderedJson::as_object)
        .cloned()
        .unwrap_or_default();
    let mut dirs: Vec<OrderedJson> = plugins
        .get("dirs")
        .and_then(|value| match value {
            OrderedJson::Array(items) => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let resolved_target = resolve_path(home, &plugin_path.to_string_lossy());
    let already = dirs.iter().any(|item| match item {
        OrderedJson::String(value) => resolve_path(home, value) == resolved_target,
        _ => false,
    });
    if !already {
        dirs.push(OrderedJson::String(
            plugin_path.to_string_lossy().into_owned(),
        ));
    }
    plugins.insert("dirs".to_owned(), OrderedJson::Array(dirs));
    let mut enabled_plugins = plugins
        .get("enabledPlugins")
        .and_then(OrderedJson::as_object)
        .cloned()
        .unwrap_or_default();
    match enabled {
        Some(value) => {
            enabled_plugins.insert(plugin_id.to_owned(), OrderedJson::Bool(value));
            plugins.insert(
                "enabledPlugins".to_owned(),
                OrderedJson::Object(enabled_plugins),
            );
        }
        None if !enabled_plugins.is_empty() => {
            plugins.insert(
                "enabledPlugins".to_owned(),
                OrderedJson::Object(enabled_plugins),
            );
        }
        None => {
            plugins.shift_remove("enabledPlugins");
        }
    }
    root.insert("plugins".to_owned(), OrderedJson::Object(plugins));
    let text =
        ordered_json::to_pretty(&OrderedJson::Object(root)).map_err(|error| error.to_string())?;
    escode_host_core::atomic_write::write_secret(&path, text.as_bytes())
        .map_err(|error| format!("{error:#}"))
}
