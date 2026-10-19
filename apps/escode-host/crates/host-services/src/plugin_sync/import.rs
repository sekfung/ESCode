//! plugin-sync 导入（TS `pluginSyncService.ts:438-538`）：解压、校验、staging、配置回写与回滚。

use std::path::Path;

use serde_json::{Value, json};

use super::archive::extract_archive;
use super::{candidates, paths};

struct ImportedPlugin {
    name: String,
    plugin_id: String,
    directory_name: String,
    enabled: Option<bool>,
}

fn read_metadata(temp: &Path) -> Result<Vec<ImportedPlugin>, String> {
    let text = std::fs::read_to_string(temp.join(super::archive::METADATA_ARCHIVE_PATH))
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let Some(object) = value.as_object() else {
        return Err("invalid plugin sync archive metadata".to_owned());
    };
    let Some(plugins) = object.get("plugins").and_then(Value::as_array) else {
        return Err("invalid plugin sync archive metadata".to_owned());
    };
    let mut result = Vec::with_capacity(plugins.len());
    for plugin in plugins {
        let Some(plugin) = plugin.as_object() else {
            return Err("invalid plugin sync archive plugin metadata".to_owned());
        };
        result.push(ImportedPlugin {
            name: required(plugin.get("name"), "plugin metadata name")?,
            plugin_id: required(plugin.get("pluginId"), "plugin metadata id")?,
            directory_name: required(plugin.get("directoryName"), "plugin metadata directory")?,
            enabled: plugin.get("enabled").and_then(Value::as_bool),
        });
    }
    Ok(result)
}

fn required(value: Option<&Value>, label: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("missing {label}"))
}

pub(super) fn import(home: &Path, archive: &[u8], max_bytes: u64) -> Result<Value, String> {
    let temp = home
        .join(".escode")
        .join("tmp")
        .join(format!("plugin-sync-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        extract_archive(archive, &temp, Some(max_bytes))?;
        let metadata = read_metadata(&temp)?;
        let root = candidates::plugin_root(home);
        let mut existing = candidates::configured_paths(home)?;
        let mut results = Vec::with_capacity(metadata.len());
        for plugin in metadata {
            let directory_name =
                paths::normalize_relative(&plugin.directory_name, "unsafe plugin archive path")?;
            let plugin_id = candidates::normalize_plugin_id(&plugin.plugin_id)?;
            let extracted =
                paths::resolve_within(&temp, &directory_name, "unsafe plugin archive path")?;
            let target =
                paths::resolve_within(&root, &directory_name, "unsafe plugin archive path")?;
            let outcome = (|| -> Result<Value, String> {
                let manifest = candidates::read_manifest(&extracted)?
                    .ok_or_else(|| format!("plugin manifest not found: {directory_name}"))?;
                if manifest.plugin_id.to_lowercase() != plugin_id.to_lowercase() {
                    return Err(format!(
                        "plugin manifest id mismatch: {} !== {plugin_id}",
                        manifest.plugin_id
                    ));
                }
                if target.exists() {
                    return Ok(json!({
                        "name": manifest.name,
                        "pluginId": plugin_id,
                        "directoryName": directory_name,
                        "status": "skipped",
                        "path": target.to_string_lossy(),
                    }));
                }
                if let Some(path) = existing.get(&plugin_id.to_lowercase()) {
                    return Ok(json!({
                        "name": manifest.name,
                        "pluginId": plugin_id,
                        "directoryName": directory_name,
                        "status": "skipped",
                        "path": path.to_string_lossy(),
                    }));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let staging = paths::resolve_within(
                    &root,
                    &format!(
                        ".importing-{}-{}",
                        candidates::safe_directory_name(&directory_name),
                        uuid::Uuid::new_v4()
                    ),
                    "unsafe plugin archive path",
                )?;
                copy_dir(&extracted, &staging)?;
                if target.exists() {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(format!(
                        "plugin target already exists: {}",
                        target.display()
                    ));
                }
                std::fs::rename(&staging, &target).map_err(|error| {
                    let _ = std::fs::remove_dir_all(&staging);
                    error.to_string()
                })?;
                if let Err(error) =
                    candidates::add_plugin_dir(home, &target, &plugin_id, plugin.enabled)
                {
                    let _ = std::fs::remove_dir_all(&target);
                    return Err(error);
                }
                existing.insert(plugin_id.to_lowercase(), target.clone());
                Ok(json!({
                    "name": manifest.name,
                    "pluginId": plugin_id,
                    "directoryName": directory_name,
                    "status": "synced",
                    "path": target.to_string_lossy(),
                }))
            })();
            results.push(match outcome {
                Ok(value) => value,
                Err(error) => json!({
                    "name": plugin.name,
                    "pluginId": plugin_id,
                    "directoryName": directory_name,
                    "status": "failed",
                    "error": error,
                }),
            });
        }
        Ok(json!({"results": results}))
    })();
    let _ = std::fs::remove_dir_all(&temp);
    result
}

fn copy_dir(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &destination)?;
        } else {
            std::fs::copy(&path, &destination).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
