//! `plugins/uninstall` / `plugins/restoreBuiltin`（docs/specs/rust-plugin-marketplace-write.md W1a）：对齐 TS
//! `uninstallPlugin` → `uninstallESCodeMarketplacePlugin`（+ adapters `uninstallMarketplacePlugin`）与
//! `restoreBuiltinPlugin` → `restoreBuiltinPluginCore`。

use super::config_file;
use super::plugin_marketplace::sanitize;
use super::{extension_config as config, extension_plugins as plugins, plugin_list};
use crate::domain::json_order::Json;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio_util::sync::CancellationToken;

/// TS `withPluginStorageLock`：按 storageRoot 串行化插件存储写操作（进程内；TS 同样不做跨进程锁）。
pub(super) async fn storage_lock(storage: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Default::default);
    let lock = LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(storage.to_owned())
        .or_default()
        .clone();
    lock.lock_owned().await
}

/// TS `normalizeInstalledPluginsState`：数组形式逐条校验（保留记录原样），Claude 风格的
/// `id → entry | entry[]` 对象形式归一化成数组（缺 installPath 的条目丢弃）。写回时一律是
/// `{version: 1, plugins: [...]}`。
pub(super) fn normalize_installed(parsed: Option<Json>) -> Vec<Json> {
    let Some(Json::Object(entries)) = parsed else {
        return vec![];
    };
    let plugins = entries
        .iter()
        .find(|(key, _)| key == "plugins")
        .map(|(_, value)| value.clone());
    match plugins {
        Some(Json::Array(items)) => items.into_iter().filter(valid_record).collect(),
        Some(Json::Object(map)) => map
            .into_iter()
            .flat_map(|(id, entry)| {
                let items = match entry {
                    Json::Array(items) => items,
                    other => vec![other],
                };
                items
                    .into_iter()
                    .filter_map(|item| from_map_entry(&id, &item))
                    .collect::<Vec<_>>()
            })
            .collect(),
        _ => vec![],
    }
}

/// TS `isInstalledPluginRecord`。
fn valid_record(record: &Json) -> bool {
    let text = |key: &str| matches!(record.get(key), Some(Json::String(_)));
    record.is_object()
        && [
            "id",
            "name",
            "marketplace",
            "version",
            "installPath",
            "installedAt",
        ]
        .iter()
        .all(|key| text(key))
        && matches!(
            record.get("scope").and_then(Json::as_str),
            Some("user" | "workspace")
        )
}

/// TS `normalizeInstalledPluginRecordFromMap`。
fn from_map_entry(id: &str, item: &Json) -> Option<Json> {
    if !item.is_object() {
        return None;
    }
    let text = |key: &str| item.get(key).and_then(Json::as_str);
    let install_path = text("installPath").filter(|p| !p.is_empty())?;
    let (name, marketplace) = id.rsplit_once('@')?;
    if name.is_empty() || marketplace.is_empty() {
        return None;
    }
    let mut record = Json::object();
    record.set("id", Json::str(id));
    record.set("name", Json::str(name));
    record.set("marketplace", Json::str(marketplace));
    record.set("version", Json::str(text("version").unwrap_or("0.0.0")));
    record.set("installPath", Json::str(install_path));
    record.set(
        "installedAt",
        Json::str(text("installedAt").unwrap_or("1970-01-01T00:00:00.000Z")),
    );
    if let Some(updated) = text("lastUpdated") {
        record.set("updatedAt", Json::str(updated));
    }
    let scope = match text("scope") {
        Some("project" | "local") => "workspace",
        _ => "user",
    };
    record.set("scope", Json::str(scope));
    Some(record)
}

pub(super) async fn read_installed(storage: &Path) -> Vec<Json> {
    let parsed = tokio::fs::read_to_string(storage.join("installed_plugins.json"))
        .await
        .ok()
        .and_then(|text| Json::parse(&text));
    normalize_installed(parsed)
}

/// 阻塞线程里用的同步读取（先按事务标记恢复，与 TS `readJsonFileSync` 同路）。
pub(super) fn read_installed_sync(storage: &Path) -> Vec<Json> {
    let path = super::atomic_dir::recover(&storage.join("installed_plugins.json"));
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| Json::parse(&text));
    normalize_installed(parsed)
}

pub(super) async fn write_installed(storage: &Path, records: Vec<Json>) -> Result<()> {
    let mut state = Json::object();
    state.set("version", Json::Number(1.into()));
    state.set("plugins", Json::Array(records));
    config_file::atomic_write(&storage.join("installed_plugins.json"), &state).await
}

fn user_config_path() -> PathBuf {
    config::home()
        .join(".escode")
        .join("cli")
        .join("config.json")
}

fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// TS `toInstalledPluginSummary(toInstalledPluginData(record, false))`（未带加载元数据）。
fn removed_summary(record: &Value) -> Value {
    let mut summary = json!({
        "id": record["id"],
        "name": record["name"],
        "marketplace": record["marketplace"],
        "enabled": false,
        "scope": record["scope"],
    });
    for key in ["version", "installPath", "installedAt"] {
        if record[key].as_str().is_some_and(|s| !s.is_empty()) {
            summary[key] = record[key].clone();
        }
    }
    summary
}

pub(super) async fn uninstall(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    // TS resolvePluginIdForMutation。
    let plugin_id = match plugin_list::non_empty(params, "pluginId") {
        Ok(id) => id.to_owned(),
        Err(_) => match (
            plugin_list::non_empty(params, "pluginName"),
            plugin_list::non_empty(params, "marketplace"),
        ) {
            (Ok(name), Ok(marketplace)) => format!("{name}@{marketplace}"),
            _ => bail!("pluginId or pluginName + marketplace is required"),
        },
    };
    // 卸载即彻底清除：除非显式 removeCache=false，否则连缓存与 data 目录一起删。
    let remove_cache = params["removeCache"] != false;
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    let user_path = user_config_path();

    // 持久化安装记录是市场所有权的权威证据，优先于运行时来源分类（官方 CDN 插件与内置插件共享 id 空间）。
    let mut records = read_installed(&storage).await;
    if let Some(index) = records
        .iter()
        .position(|record| record.get("id").and_then(Json::as_str) == Some(plugin_id.as_str()))
    {
        let removed = records.remove(index);
        write_installed(&storage, records).await?;
        let removed: Value = serde_json::from_str(&removed.compact())?;
        if remove_cache {
            if let Some(path) = removed["installPath"].as_str() {
                remove_dir(Path::new(path)).await?;
            }
            remove_dir(&storage.join("data").join(sanitize(&plugin_id))).await?;
        }
        config_file::patch_file(&user_path, |file| {
            config_file::remove_plugin(file, &plugin_id)
        })
        .await?;
        config_file::patch_file(&user_path, |file| {
            config_file::remove_suppressed_builtin(file, &plugin_id)
        })
        .await?;
        return Ok(json!({ "removedPlugin": removed_summary(&removed), "diagnostics": [] }));
    }

    // 内置（官方）插件不在 installed_plugins.json 里：只写抑制标记、清用户配置与数据目录，
    // 官方缓存保留（详情页离线读组件、恢复不依赖重新下载）。
    let discovered = plugins::all(&cwd, &config, cancel).await?;
    if let Some(builtin) = discovered
        .iter()
        .find(|plugin| plugin.id == plugin_id && plugin.source == "official")
    {
        config_file::patch_file(&user_path, |file| {
            config_file::add_suppressed_builtin(file, &plugin_id)
        })
        .await?;
        config_file::patch_file(&user_path, |file| {
            config_file::remove_plugin(file, &plugin_id)
        })
        .await?;
        remove_dir(&storage.join("data").join(sanitize(&plugin_id))).await?;
        let record = json!({
            "id": builtin.id,
            "name": builtin.name,
            "marketplace": builtin.marketplace,
            "version": builtin.manifest["version"],
            "installPath": builtin.root.to_string_lossy(),
            "installedAt": iso_now(),
            "scope": "user",
        });
        return Ok(json!({ "removedPlugin": removed_summary(&record), "diagnostics": [] }));
    }
    Ok(json!({ "diagnostics": [] }))
}

/// `rm -rf`，不存在不报错（TS `rm({force: true, recursive: true})`）。
async fn remove_dir(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("Unable to remove {}", path.display())))
        }
    }
}

const COMPUTER_USE: &str = "computer-use@escode-plugins-official";

/// 清掉抑制标记并立即重新 seed；computer-use 需要 CUA 特性（overview 隐藏入口，协议直调也在写盘前拒绝）。
pub(super) async fn restore_builtin(params: &Value) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let plugin_id = plugin_list::non_empty(params, "pluginId")?.to_owned();
    if plugin_id == COMPUTER_USE && !super::plugin_overview::cua_feature_enabled() {
        bail!("computer-use built-in plugin requires ESCODE_CUA_PRODUCT_HELPER to be enabled");
    }
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    config_file::patch_file(&user_config_path(), |file| {
        config_file::remove_suppressed_builtin(file, &plugin_id)
    })
    .await?;
    let seed_root = storage.clone();
    tokio::task::spawn_blocking(move || super::official_plugins::reseed(&seed_root))
        .await
        .context("Official plugin reseed panicked")?;
    Ok(json!({ "pluginId": plugin_id, "diagnostics": [] }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_state_normalizes_both_shapes() {
        let array = Json::parse(
            r#"{"version":1,"plugins":[{"id":"a@m","name":"a","marketplace":"m","version":"1","installPath":"/p","installedAt":"t","scope":"user","extra":1},{"id":"bad"}]}"#,
        );
        let records = normalize_installed(array);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].get("extra"), Some(&Json::Number(1.into())));
        let map = Json::parse(
            r#"{"plugins":{"b@m":[{"installPath":"/b","scope":"project","lastUpdated":"u"}],"c@m":{"version":"2"}}}"#,
        );
        let records = normalize_installed(map);
        assert_eq!(
            records[0].compact(),
            r#"{"id":"b@m","name":"b","marketplace":"m","version":"0.0.0","installPath":"/b","installedAt":"1970-01-01T00:00:00.000Z","updatedAt":"u","scope":"workspace"}"#
        );
        assert_eq!(records.len(), 1);
    }
}
