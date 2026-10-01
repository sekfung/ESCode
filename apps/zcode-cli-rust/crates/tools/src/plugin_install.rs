//! `plugins/install`（docs/specs/rust-plugin-marketplace-write.md W1b：本地源）：对齐 TS `installPlugin` →
//! `installZCodeMarketplacePlugin` → adapters `installMarketplacePlugin` / `cacheMarketplacePlugin`。
//! 远端源（zip / git / github）在 W2 / W3 实现之前按 TS「recognized but not supported」同一条诊断返回。

use super::config_file;
use super::plugin_marketplace::{self as market, OFFICIAL_MARKETPLACE};
use super::plugin_uninstall::{read_installed_sync, storage_lock};
use super::{extension_config as config, extension_plugins as plugins, plugin_list};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub(super) use super::plugin_install_cache::{ensure_entry_manifest, install_closure};
#[allow(unused_imports)]
pub(super) use super::plugin_install_entry::{
    cache_dir, closure, closure_in, dependencies, entry_in, installed_version, manifest_path,
    normalized_dependencies, ordered_entry, ordered_manifest, resolve_inside, split_id,
    valid_plugin_name,
};
#[allow(unused_imports)]
pub(super) use super::plugin_install_source::{
    SourceRoot, assert_zip_root, is_zip_source, materialize, repository_source, source_root,
    source_root_in,
};

pub(super) async fn install(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let name = plugin_list::non_empty(params, "pluginName")?.to_owned();
    let marketplace = plugin_list::non_empty(params, "marketplace")?.to_owned();
    plugin_list::scope_of(params)?;
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    if params["dryRun"] == true {
        market::ensure_default_marketplaces(&storage)?;
        return super::plugin_validate::install_dry_run(&storage, &marketplace, &name).await;
    }
    install_locked(&cwd, &config, &storage, &name, &marketplace, cancel).await
}

/// TS `updatePlugin`（协议）：按 pluginId / marketplace 过滤已安装记录，在同一把存储锁里逐个按原市场重装，
/// 聚合安装结果、闭包与诊断（失败不抛错，诊断随结果返回）。
pub(super) async fn update(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let plugin_id = plugin_list::non_empty(params, "pluginId")
        .ok()
        .map(str::to_owned);
    let marketplace = plugin_list::non_empty(params, "marketplace")
        .ok()
        .map(str::to_owned);
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    let records: Vec<(String, String)> = read_installed_sync(&storage)
        .iter()
        .filter(|record| {
            let field = |key: &str| record.get(key).and_then(Json::as_str).unwrap_or_default();
            match (&plugin_id, &marketplace) {
                (Some(id), _) => field("id") == id,
                (None, Some(market)) => field("marketplace") == market,
                (None, None) => true,
            }
        })
        .map(|record| {
            let field = |key: &str| {
                record
                    .get(key)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            (field("name"), field("marketplace"))
        })
        .collect();
    let mut installed = vec![];
    let mut closure = vec![];
    let mut diagnostics = vec![];
    for (name, market) in records {
        let config = config::load(&cwd).await?;
        let result = install_locked(&cwd, &config, &storage, &name, &market, cancel).await?;
        for (key, out) in [
            ("installedPlugins", &mut installed),
            ("dependencyClosure", &mut closure),
            ("diagnostics", &mut diagnostics),
        ] {
            out.extend(result[key].as_array().cloned().unwrap_or_default());
        }
    }
    Ok(
        json!({ "dependencyClosure": closure, "installedPlugins": installed, "diagnostics": diagnostics }),
    )
}

pub(super) const CANCELLED: &str = "Plugin operation cancelled";

/// 安装一个插件（调用方已持有存储锁）。
async fn install_locked(
    cwd: &Path,
    config: &Value,
    storage: &Path,
    name: &str,
    marketplace: &str,
    cancel: &CancellationToken,
) -> Result<Value> {
    let (cwd, storage) = (cwd.to_owned(), storage.to_owned());
    let (name, marketplace) = (name.to_owned(), marketplace.to_owned());
    market::ensure_default_marketplaces(&storage)?;
    let plugin_id = format!("{name}@{marketplace}");
    let user_path = config::home()
        .join(".zcode")
        .join("cli")
        .join("config.json");

    // 被抑制的内置官方插件（filesystem / sea 源）只是目录指针：复用 restore，不写 installed record。
    let bundled_source = ordered_entry(&storage, &marketplace, &name).and_then(|entry| {
        entry
            .get("source")
            .and_then(Json::as_str)
            .map(str::to_owned)
    });
    let suppressed =
        config::strings(&config["plugins"]["suppressedBuiltins"]).contains(&plugin_id.as_str());
    if marketplace == OFFICIAL_MARKETPLACE
        && suppressed
        && matches!(bundled_source.as_deref(), Some("filesystem" | "sea"))
    {
        if plugin_id == "computer-use@zcode-plugins-official"
            && !super::plugin_overview::cua_feature_enabled()
        {
            bail!("computer-use built-in plugin requires ZCODE_CUA_PRODUCT_HELPER to be enabled");
        }
        return restore_bundled(&cwd, &storage, &plugin_id, &user_path, cancel).await;
    }

    // TS ensureMarketplaceManifestAvailable：本地没有目录 manifest 但有已知记录时先拉取（失败归入安装诊断）。
    let ensured = if cancel.is_cancelled() {
        Err(anyhow!(CANCELLED))
    } else {
        super::plugin_market_write::ensure_manifest(&storage, &marketplace).await
    };
    if let Err(error) = ensured {
        return Ok(json!({
            "dependencyClosure": [],
            "installedPlugins": [],
            "diagnostics": [install_diagnostic(&error, &plugin_id)],
        }));
    }
    let worker_storage = storage.clone();
    let (worker_market, worker_name) = (marketplace.clone(), name.clone());
    let worker_cancel = cancel.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        install_closure(
            &worker_storage,
            &worker_market,
            &worker_name,
            &worker_cancel,
        )
    })
    .await
    .map_err(|_| anyhow!("Plugin install worker panicked"))?;
    let (closure, records) = match outcome {
        Ok(done) => done,
        Err(error) => {
            return Ok(json!({
                "dependencyClosure": [],
                "installedPlugins": [],
                "diagnostics": [install_diagnostic(&error, &plugin_id)],
            }));
        }
    };
    let ids: Vec<String> = records
        .iter()
        .filter_map(|r| r.get("id").and_then(Json::as_str).map(str::to_owned))
        .collect();
    if marketplace == OFFICIAL_MARKETPLACE {
        // 同名 CDN 插件重装：清掉历史内置 suppression，否则运行时仍判为被抑制。
        for id in &ids {
            config_file::patch_file(&user_path, |file| {
                config_file::remove_suppressed_builtin(file, id)
            })
            .await?;
        }
    }
    // 安装即默认启用：只给用户配置里尚未显式声明的 id 写 true（停用后重装不被覆盖）。
    let mut newly_enabled: Vec<String> = vec![];
    config_file::patch_file(&user_path, |file| {
        newly_enabled = config_file::enable_by_default(file, &ids);
        !newly_enabled.is_empty()
    })
    .await?;
    let installed: Vec<Value> = records
        .iter()
        .map(|record| {
            let id = record.get("id").and_then(Json::as_str).unwrap_or_default();
            let enabled = newly_enabled.iter().any(|n| n == id)
                || config["plugins"]["enabledPlugins"][id]
                    .as_bool()
                    .unwrap_or(false);
            summary(
                &serde_json::from_str(&record.compact()).unwrap_or_default(),
                enabled,
            )
        })
        .collect();
    Ok(json!({ "dependencyClosure": closure, "installedPlugins": installed, "diagnostics": [] }))
}

/// TS `toInstalledPluginSummary(toInstalledPluginData(record, enabled))`（未带加载元数据）。
fn summary(record: &Value, enabled: bool) -> Value {
    let mut out = json!({
        "id": record["id"],
        "name": record["name"],
        "marketplace": record["marketplace"],
        "enabled": enabled,
        "scope": record["scope"],
    });
    for key in ["version", "installPath", "installedAt"] {
        if record[key].as_str().is_some_and(|s| !s.is_empty()) {
            out[key] = record[key].clone();
        }
    }
    out
}

async fn restore_bundled(
    cwd: &Path,
    storage: &Path,
    plugin_id: &str,
    user_path: &Path,
    cancel: &CancellationToken,
) -> Result<Value> {
    config_file::patch_file(user_path, |file| {
        config_file::remove_suppressed_builtin(file, plugin_id)
    })
    .await?;
    let seed_root = storage.to_owned();
    tokio::task::spawn_blocking(move || super::official_plugins::reseed(&seed_root))
        .await
        .map_err(|_| anyhow!("Official plugin reseed panicked"))?;
    let fresh = config::load(cwd).await?;
    let discovered = plugins::all(cwd, &fresh, cancel).await?;
    let Some(restored) = discovered.iter().find(|p| p.id == plugin_id) else {
        return Ok(json!({
            "dependencyClosure": [],
            "installedPlugins": [],
            "diagnostics": [{
                "code": "plugin_not_found",
                "message": format!("Bundled plugin could not be restored: {plugin_id}"),
                "severity": "error",
                "pluginId": plugin_id,
            }],
        }));
    };
    let data_root = config::storage(&fresh).join("data");
    let info = plugin_list::info(restored, cwd, &fresh, &data_root).await?;
    let mut item = summary(
        &json!({
            "id": restored.id,
            "name": restored.name,
            "marketplace": restored.marketplace,
            "version": restored.manifest["version"],
            "installPath": restored.root.to_string_lossy(),
            "installedAt": iso_now(),
            "scope": "user",
        }),
        restored.enabled,
    );
    if let Some(description) = restored.manifest["description"]
        .as_str()
        .filter(|d| !d.is_empty())
    {
        item["description"] = description.into();
    }
    item["componentTypes"] = super::plugin_overview::component_types_from_info(&info);
    Ok(json!({ "dependencyClosure": [plugin_id], "installedPlugins": [item], "diagnostics": [] }))
}

pub(super) fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// TS `toMarketplaceInstallDiagnostic`：按错误文案归类。
fn install_diagnostic(error: &anyhow::Error, plugin_id: &str) -> Value {
    let message = error.to_string();
    let message = message.as_str();
    let source_code = error
        .downcast_ref::<super::plugin_git::SourceError>()
        .map(|e| e.code);
    let code = if let Some(code) = source_code {
        code
    } else if message.starts_with("Plugin not found:") {
        "plugin_not_found"
    } else if message.contains("Cross-marketplace dependency") {
        "plugin_dependency_cross_marketplace"
    } else if message.contains("dependency cycle") {
        "plugin_dependency_cycle"
    } else if message.contains("Dependency not found")
        || message.contains("Marketplace not found for dependency")
    {
        "plugin_dependency_missing"
    } else if message.contains("source is recognized but not supported") {
        "plugin_marketplace_source_unsupported"
    } else {
        "plugin_marketplace_invalid"
    };
    json!({ "code": code, "message": message, "severity": "error", "pluginId": plugin_id })
}

// ---- 阻塞线程里的存储工作 ----
