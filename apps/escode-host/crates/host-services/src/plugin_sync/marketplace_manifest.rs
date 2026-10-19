//! marketplace manifest 解析与依赖闭包（TS `pluginSyncService.ts:210-421,592-733`）。

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::Value;

use crate::ordered_json::OrderedJson;

use super::candidates;

#[derive(Clone)]
pub struct PluginEntry {
    pub name: String,
    pub dependencies: Vec<String>,
    pub raw: IndexMap<String, OrderedJson>,
    pub source: Option<OrderedJson>,
}

pub struct Manifest {
    pub name: String,
    pub description: Option<String>,
    pub plugin_root: Option<String>,
    pub plugins: Vec<PluginEntry>,
    pub raw: IndexMap<String, OrderedJson>,
}

pub(super) struct Loaded {
    pub(super) manifest: Manifest,
    pub(super) source_root: Option<PathBuf>,
}

pub(super) fn load_source(
    source: &serde_json::Map<String, Value>,
    home: &Path,
) -> Result<Loaded, String> {
    let kind = source.get("source").and_then(Value::as_str).unwrap_or("");
    match kind {
        "file" => {
            let path = require_string(source.get("path"), "marketplace source file path")?;
            let path = candidates::resolve_path(home, &path);
            Ok(Loaded {
                source_root: path.parent().map(Path::to_path_buf),
                manifest: read_manifest(&path)?,
            })
        }
        "directory" => {
            let root = require_string(source.get("path"), "marketplace source directory path")?;
            let root = candidates::resolve_path(home, &root);
            let manifest_path = [
                root.join("marketplace.json"),
                root.join(".claude-plugin").join("marketplace.json"),
            ]
            .into_iter()
            .find(|path| path.exists())
            .ok_or_else(|| {
                format!(
                    "marketplace manifest not found in directory: {}",
                    root.display()
                )
            })?;
            Ok(Loaded {
                manifest: read_manifest(&manifest_path)?,
                source_root: Some(root),
            })
        }
        "settings" => {
            let raw = source
                .get("marketplace")
                .ok_or("settings marketplace source is missing marketplace manifest")?;
            Ok(Loaded {
                manifest: parse_manifest(raw)?,
                source_root: None,
            })
        }
        other => Err(format!(
            "unsupported marketplace source for SSH sync: {}",
            if other.is_empty() { "unknown" } else { other }
        )),
    }
}

pub(super) fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    parse_manifest(&value)
}

fn parse_manifest(value: &Value) -> Result<Manifest, String> {
    let Some(object) = value.as_object() else {
        return Err("marketplace manifest is invalid".to_owned());
    };
    let name = require_string(object.get("name"), "marketplace name")?;
    let plugin_root = object
        .get("pluginRoot")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            object
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get("pluginRoot"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_owned);
    let raw_plugins = object.get("plugins");
    let plugin_values: Vec<Value> = match raw_plugins {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(plugin_name, plugin)| match plugin {
                Value::Object(plugin) => {
                    let mut merged = plugin.clone();
                    merged
                        .entry("name".to_owned())
                        .or_insert_with(|| Value::String(plugin_name.clone()));
                    Value::Object(merged)
                }
                _ => serde_json::json!({"name": plugin_name}),
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut plugins = Vec::new();
    for plugin in plugin_values {
        let Value::Object(plugin) = plugin else {
            continue;
        };
        let plugin_name = require_string(plugin.get("name"), "marketplace plugin name")?;
        let dependencies = plugin
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|item| !item.trim().is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let raw = OrderedJson::from_value(&Value::Object(plugin.clone()));
        plugins.push(PluginEntry {
            name: plugin_name,
            dependencies,
            source: plugin.get("source").map(OrderedJson::from_value),
            raw: match raw {
                OrderedJson::Object(map) => map,
                _ => IndexMap::new(),
            },
        });
    }
    Ok(Manifest {
        name,
        description: object
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        plugin_root,
        plugins,
        raw: match OrderedJson::from_value(value) {
            OrderedJson::Object(map) => map,
            _ => IndexMap::new(),
        },
    })
}

pub(super) fn closure(
    manifest: &Manifest,
    marketplace_id: &str,
    names: &[String],
) -> Result<Vec<PluginEntry>, String> {
    let by_name: std::collections::BTreeMap<&str, &PluginEntry> = manifest
        .plugins
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    fn visit(
        name: &str,
        required_by: &str,
        marketplace_id: &str,
        by_name: &std::collections::BTreeMap<&str, &PluginEntry>,
        visited: &mut std::collections::BTreeSet<String>,
        result: &mut Vec<PluginEntry>,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("missing marketplace plugin name".to_owned());
        }
        if !visited.insert(name.to_owned()) {
            return Ok(());
        }
        let entry = by_name
            .get(name)
            .ok_or_else(|| format!("marketplace plugin not found: {name}"))?;
        result.push((*entry).clone());
        for dependency in &entry.dependencies {
            let dependency = normalize_dependency(dependency, marketplace_id, required_by)?;
            visit(
                &dependency,
                &entry.name,
                marketplace_id,
                by_name,
                visited,
                result,
            )?;
        }
        Ok(())
    }
    for name in names {
        visit(
            name,
            name,
            marketplace_id,
            &by_name,
            &mut visited,
            &mut result,
        )?;
    }
    Ok(result)
}

fn normalize_dependency(
    value: &str,
    marketplace_id: &str,
    required_by: &str,
) -> Result<String, String> {
    let trimmed = value.trim();
    let Some(index) = trimmed.rfind('@') else {
        return Ok(trimmed.to_owned());
    };
    let name = &trimmed[..index];
    let marketplace = &trimmed[index + 1..];
    if marketplace == marketplace_id {
        Ok(name.to_owned())
    } else {
        Err(format!(
            "cross-marketplace dependency is not supported for SSH source mirror: {value} required by {required_by}"
        ))
    }
}

pub(super) fn require_string(value: Option<&Value>, label: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("missing {label}"))
}
