//! 插件的安装记录与官方缓存扫描（TS `officialPluginCache.ts` + `readInstalledPluginRecords` 一族）。
//!
//! 与 `commands` 的缓存扫描**不是同一套**：这里按插件名分组返回 `{name, versionRoots}`，
//! 版本目录做「数字感知、忽略大小写、降序」排序并过滤瞬态目录。
//!
//! 两条容易漏的规则：
//! 1. **卸载抑制优先于遗留安装记录**：`installed_plugins.json` 里被 `suppressedBuiltins` 抑制的记录不能用；
//! 2. 内置插件没有安装记录，靠缓存兜底，但缓存**不能复活**被抑制的插件。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::discovery_plugin::PluginRecord;
use super::storage;
use crate::official_plugins::{OFFICIAL_PLUGIN_MARKETPLACE, is_default_enabled};

/// 插件清单的三种目录格式，按序取第一个能解析成对象的。
const MANIFEST_PATHS: [&str; 3] = [
    ".escode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".zcode-plugin/plugin.json",
];

fn is_transient(name: &str) -> bool {
    name.contains(".backup") || name.contains(".seed-lock") || name.contains(".tmp-")
}

/// 数字感知 + 忽略大小写的自然序（TS `localeCompare(..., {numeric, sensitivity:"base"})` 的近似）。
/// 登记差异：ICU 的完整排序表未复刻，只在多版本缓存目录的排序上可见。
fn natural_cmp(left: &str, right: &str) -> Ordering {
    fn runs(value: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for character in value.chars() {
            let digit = character.is_ascii_digit();
            match out.last_mut() {
                Some((flag, text)) if *flag == digit => text.push(character),
                _ => out.push((digit, character.to_string())),
            }
        }
        out
    }
    let (left_runs, right_runs) = (runs(left), runs(right));
    for (index, (is_digit, text)) in left_runs.iter().enumerate() {
        let Some((_, other)) = right_runs.get(index) else {
            return Ordering::Greater;
        };
        let ordering = if *is_digit {
            let left_number: u128 = text.parse().unwrap_or(0);
            let right_number: u128 = other.parse().unwrap_or(0);
            left_number.cmp(&right_number)
        } else {
            text.to_lowercase().cmp(&other.to_lowercase())
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left_runs.len().cmp(&right_runs.len())
}

/// TS `scanOfficialPluginCacheRoots`（services 版）：按插件名分组，版本目录数字感知降序。
pub(super) async fn scan_official_cache(plugin_storage: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let cache_root = plugin_storage
        .join("cache")
        .join(OFFICIAL_PLUGIN_MARKETPLACE);
    let Ok(mut plugins) = tokio::fs::read_dir(&cache_root).await else {
        return Vec::new();
    };
    let mut roots: Vec<(String, Vec<PathBuf>)> = Vec::new();
    while let Ok(Some(plugin)) = plugins.next_entry().await {
        let Ok(file_type) = plugin.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = plugin.file_name().to_string_lossy().into_owned();
        let plugin_dir = plugin.path();
        let Ok(mut versions) = tokio::fs::read_dir(&plugin_dir).await else {
            continue;
        };
        let mut version_names: Vec<String> = Vec::new();
        while let Ok(Some(version)) = versions.next_entry().await {
            let Ok(file_type) = version.file_type().await else {
                continue;
            };
            let version_name = version.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() && !is_transient(&version_name) {
                version_names.push(version_name);
            }
        }
        if version_names.is_empty() {
            continue;
        }
        version_names.sort_by(|left, right| natural_cmp(right, left));
        roots.push((
            name,
            version_names
                .into_iter()
                .map(|version| plugin_dir.join(version))
                .collect(),
        ));
    }
    roots.sort_by(|(left, _), (right, _)| left.cmp(right));
    roots
}

pub(super) async fn read_installed_records(plugin_storage: &Path) -> Vec<PluginRecord> {
    let Ok(raw) = tokio::fs::read_to_string(plugin_storage.join("installed_plugins.json")).await
    else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(plugins) = parsed.get("plugins").and_then(Value::as_array) else {
        return Vec::new();
    };
    plugins
        .iter()
        .filter_map(|item| {
            let object = item.as_object()?;
            Some(PluginRecord {
                id: object.get("id")?.as_str()?.to_owned(),
                install_path: PathBuf::from(object.get("installPath")?.as_str()?),
                name: object.get("name")?.as_str()?.to_owned(),
            })
        })
        .collect()
}

pub struct PipelineConfig {
    pub enabled_plugins: BTreeMap<String, bool>,
    pub suppressed_builtins: Vec<String>,
}

pub async fn read_plugin_config(home: &Path) -> PipelineConfig {
    let config = storage::read_user_cli_config(home).await;
    let plugins = config.get("plugins").filter(|value| value.is_object());
    let mut enabled_plugins = BTreeMap::new();
    if let Some(entries) = plugins
        .and_then(|plugins| plugins.get("enabledPlugins"))
        .and_then(Value::as_object)
    {
        for (id, value) in entries {
            if let Some(flag) = value.as_bool() {
                enabled_plugins.insert(id.clone(), flag);
            }
        }
    }
    let suppressed_builtins = plugins
        .and_then(|plugins| plugins.get("suppressedBuiltins"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    PipelineConfig {
        enabled_plugins,
        suppressed_builtins,
    }
}

/// TS `readEnabledPluginRecords`：安装记录过滤 + 官方缓存兜底。
pub async fn read_enabled_records(
    plugin_storage: &Path,
    config: &PipelineConfig,
) -> Vec<PluginRecord> {
    let installed = read_installed_records(plugin_storage).await;
    let mut records: Vec<PluginRecord> = installed
        .iter()
        .filter(|record| {
            config.enabled_plugins.get(&record.id).copied() == Some(true)
                && !(record
                    .id
                    .ends_with(&format!("@{OFFICIAL_PLUGIN_MARKETPLACE}"))
                    && config.suppressed_builtins.contains(&record.id))
        })
        .cloned()
        .collect();
    let seen: BTreeSet<String> = installed.iter().map(|record| record.id.clone()).collect();
    let mut seen = seen;
    for (name, version_roots) in scan_official_cache(plugin_storage).await {
        let id = format!("{name}@{OFFICIAL_PLUGIN_MARKETPLACE}");
        if seen.contains(&id) || config.suppressed_builtins.contains(&id) {
            continue;
        }
        let enabled = config
            .enabled_plugins
            .get(&id)
            .copied()
            .unwrap_or_else(|| is_default_enabled(&id));
        if !enabled {
            continue;
        }
        for version_root in version_roots {
            let Some(manifest) = read_manifest(&version_root).await else {
                continue;
            };
            if manifest.get("name").and_then(Value::as_str) != Some(name.as_str()) {
                continue;
            }
            records.push(PluginRecord {
                id: id.clone(),
                install_path: version_root,
                name: name.clone(),
            });
            seen.insert(id.clone());
            break;
        }
    }
    records
}

/// TS `readPluginManifest`：三种清单格式按序取第一个能解析成对象的。
pub(super) async fn read_manifest(root: &Path) -> Option<Value> {
    for relative in MANIFEST_PATHS {
        let Ok(raw) = tokio::fs::read_to_string(root.join(relative)).await else {
            continue;
        };
        if let Ok(parsed) = serde_json::from_str::<Value>(&raw)
            && parsed.is_object()
        {
            return Some(parsed);
        }
    }
    None
}
