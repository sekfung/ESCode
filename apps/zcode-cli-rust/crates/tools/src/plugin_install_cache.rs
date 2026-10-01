//! 安装事务（TS `installMarketplacePlugin`）：解析闭包 → 逐个激活缓存 → 写 installed_plugins.json，失败逆序回滚。
#[allow(unused_imports)]
use super::plugin_install::*;

use super::atomic_dir;
use super::plugin_uninstall::read_installed_sync;
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// TS `ensureMarketplaceEntryManifest`：目标没有 plugin.json 且条目 `strict: false` 时合成
/// `.claude-plugin/plugin.json`（剔除来源 / 商店展示字段，补 name / version）。
pub(super) fn ensure_entry_manifest(entry: &Json, target: &Path) -> Result<()> {
    if manifest_path(target).is_some() || entry.get("strict") != Some(&Json::Bool(false)) {
        return Ok(());
    }
    let mut manifest = entry.clone();
    for key in [
        "source",
        "category",
        "tags",
        "strict",
        "displayName",
        "displayName_i18n",
        "description_i18n",
        "icon",
        "privacyPolicy",
        "termsOfService",
        "heroImage",
        "examplePrompts",
        "examplePrompts_i18n",
        "requiresPaidPlan",
    ] {
        manifest.remove(key);
    }
    let name = entry
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    manifest.set("name", Json::str(name));
    let version = entry
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or("0.0.0")
        .to_owned();
    manifest.set("version", Json::str(version));
    let dir = target.join(".claude-plugin");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("plugin.json"), format!("{}\n", manifest.pretty()))?;
    Ok(())
}

/// TS `installMarketplacePlugin`：解析闭包 → 逐个激活缓存 → 写 installed_plugins.json；
/// 任一步失败按逆序回滚已激活目录。
pub(super) fn install_closure(
    storage: &Path,
    marketplace: &str,
    name: &str,
    cancel: &CancellationToken,
) -> Result<(Vec<String>, Vec<Json>)> {
    let ids = closure(storage, marketplace, name)?;
    let mut state = read_installed_sync(storage);
    let authority = storage.join("installed_plugins.json");
    let mut installed = vec![];
    let mut activations = vec![];
    let now = iso_now();
    let result = (|| -> Result<()> {
        for id in &ids {
            // 安全点：每个插件物化前检查取消（TS throwIfPluginOperationAborted）。
            if cancel.is_cancelled() {
                bail!(CANCELLED);
            }
            let (plugin_name, plugin_market) = split_id(id)?;
            let entry = ordered_entry(storage, plugin_market, plugin_name)
                .ok_or_else(|| anyhow!("Plugin not found: {id}"))?;
            // zip 源（W2）：下载解压到临时目录；激活后（无论成败）清理。
            // 远端源（W2 zip / W3 仓库）物化到临时目录；激活后（无论成败）清理。
            let root = materialize(storage, plugin_market, &entry, None)?;
            if root.zip
                && let Err(error) = assert_zip_root(&root.path, &entry, plugin_market)
            {
                root.cleanup();
                return Err(error);
            }
            let source = root.path.clone();
            let version = installed_version(&source, &entry);
            let target = cache_dir(storage, plugin_market, plugin_name, &version);
            let same =
                super::lexical_path::normalize(&source) == super::lexical_path::normalize(&target);
            let mut transaction = None;
            let activated = if same {
                ensure_entry_manifest(&entry, &target).map(|_| None)
            } else {
                atomic_dir::activate(Some(&source), &target, &authority, |staged| {
                    ensure_entry_manifest(&entry, staged)
                })
                .map(Some)
            };
            // 缓存已复制（或失败）后临时目录清理失败不阻断安装记录落盘。
            root.cleanup();
            if let Some(activation) = activated? {
                transaction = Some(activation.transaction_id.clone());
                activations.push(activation);
            }
            let mut record = Json::object();
            record.set("id", Json::str(id.clone()));
            record.set("name", Json::str(plugin_name));
            record.set("marketplace", Json::str(plugin_market));
            record.set("version", Json::str(version.clone()));
            record.set("installPath", Json::str(target.to_string_lossy()));
            record.set("installedAt", Json::str(now.clone()));
            record.set("updatedAt", Json::str(now.clone()));
            record.set("scope", Json::str("user"));
            let deps = normalized_dependencies(&entry);
            if entry
                .get("dependencies")
                .is_some_and(|d| d.as_array().is_some())
            {
                record.set(
                    "dependencies",
                    Json::Array(deps.into_iter().map(Json::String).collect()),
                );
            }
            if let Some(source) = entry.get("source") {
                record.set("source", source.clone());
            }
            if let Some(transaction) = transaction {
                record.set("cacheTransactionId", Json::str(transaction));
            }
            // 已有记录：原地覆盖（去掉旧 cacheTransactionId），保留首次 installedAt。
            match state
                .iter_mut()
                .find(|existing| existing.get("id").and_then(Json::as_str) == Some(id.as_str()))
            {
                Some(existing) => {
                    let first = existing
                        .get("installedAt")
                        .cloned()
                        .unwrap_or_else(|| Json::str(now.clone()));
                    existing.remove("cacheTransactionId");
                    if let Json::Object(fields) = &record {
                        for (key, value) in fields {
                            existing.set(key, value.clone());
                        }
                    }
                    existing.set("installedAt", first);
                    installed.push(existing.clone());
                }
                None => {
                    state.push(record.clone());
                    installed.push(record);
                }
            }
        }
        // 提交点之前最后一次检查；之后写权威状态并落定，不再响应取消。
        if cancel.is_cancelled() {
            bail!(CANCELLED);
        }
        let mut file = Json::object();
        file.set("version", Json::Number(1.into()));
        file.set("plugins", Json::Array(state.clone()));
        std::fs::create_dir_all(storage)?;
        let temp = storage.join(format!(
            ".installed_plugins.json.stage-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&temp, format!("{}\n", file.pretty()))?;
        std::fs::rename(&temp, &authority).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_error = None;
        for activation in activations.into_iter().rev() {
            if let Err(e) = activation.rollback() {
                rollback_error.get_or_insert(e);
            }
        }
        return Err(match rollback_error {
            Some(rollback) => error.context(format!("cleanup also failed: {rollback}")),
            None => error,
        });
    }
    for activation in activations {
        activation.finalize();
    }
    Ok((ids, installed))
}
