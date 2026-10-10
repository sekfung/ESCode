//! 插件市场写面（docs/specs/rust-plugin-marketplace-write.md W4）：`plugins/marketplace/add|remove|update` 与安装时的
//! 按需拉取。对齐 TS adapters `addMarketplace` / `updateMarketplace` / `removeMarketplace` 与 bootstrap
//! `addESCodePluginMarketplace` / `updateESCodePluginMarketplace`。

use super::plugin_marketplace::{OFFICIAL_MARKETPLACE, sanitize};
use super::{atomic_dir, plugin_git};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub(super) use super::plugin_market_params::{
    add_params, ensure_manifest, remove_params, update_params,
};
#[allow(unused_imports)]
pub(super) use super::plugin_market_source::{
    Loaded, UNSUPPORTED, load, normalize, parse_source_input,
};

pub(super) const CANCELLED: &str = "Plugin operation cancelled";

pub(super) fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

pub(super) fn known_path(storage: &Path) -> PathBuf {
    storage.join("known_marketplaces.json")
}

pub(super) fn valid_known(record: &Json) -> bool {
    record.is_object()
        && matches!(record.get("id"), Some(Json::String(_)))
        && matches!(record.get("name"), Some(Json::String(_)))
        && matches!(record.get("pluginCount"), Some(Json::Number(_)))
        && record.get("source").is_some_and(Json::is_object)
}

/// TS `loadKnownMarketplacesSync`（保序）：`marketplaces` 为数组或对象，只保留合法记录。
pub(super) fn known(storage: &Path) -> Vec<Json> {
    let path = atomic_dir::recover(&known_path(storage));
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| Json::parse(&text));
    let records = match parsed.as_ref().and_then(|p| p.get("marketplaces")) {
        Some(Json::Array(items)) => items.clone(),
        Some(Json::Object(entries)) => entries.iter().map(|(_, v)| v.clone()).collect(),
        _ => vec![],
    };
    records.into_iter().filter(valid_known).collect()
}

/// TS `writeKnownMarketplaces`（`writeJsonFile` → 原子替换）。
pub(super) fn write_known(storage: &Path, records: Vec<Json>) -> Result<()> {
    let mut file = Json::object();
    file.set("version", Json::Number(1.into()));
    file.set("marketplaces", Json::Array(records));
    std::fs::create_dir_all(storage)?;
    let target = known_path(storage);
    let temp = storage.join(format!(
        ".known_marketplaces.json.stage-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&temp, format!("{}\n", file.pretty()))?;
    std::fs::rename(&temp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    Ok(())
}

pub(super) fn id_of(record: &Json) -> &str {
    record.get("id").and_then(Json::as_str).unwrap_or_default()
}

// ---- 源输入解析（TS parseMarketplaceSourceInput） ----

/// TS `addMarketplace`：加载 → 官方 id 守卫 → 激活目录（与 known 记录同一 transactionId）→ upsert known。
pub(super) async fn add(
    storage: &Path,
    source: &Json,
    expected_id: Option<&str>,
    trusted_id: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Json> {
    if cancel.is_cancelled() {
        bail!(CANCELLED);
    }
    let loaded = load(source).await?;
    // 提交点之前的最后安全点：之后的激活与 known 写入不再响应取消。
    let result = if cancel.is_cancelled() {
        Err(anyhow!(CANCELLED))
    } else {
        commit(storage, source, &loaded, expected_id, trusted_id)
    };
    if let Some(temp) = &loaded.temp {
        let _ = std::fs::remove_dir_all(temp);
    }
    result
}

pub(super) fn commit(
    storage: &Path,
    source: &Json,
    loaded: &Loaded,
    expected_id: Option<&str>,
    trusted_id: Option<&str>,
) -> Result<Json> {
    let name = loaded.name.as_str();
    if name == OFFICIAL_MARKETPLACE && Some(name) != trusted_id {
        bail!(
            "Cannot add a marketplace named \"{name}\": that id is reserved for the official marketplace."
        );
    }
    if let Some(expected) = expected_id
        && name != expected
    {
        bail!("Marketplace declaration id mismatch: expected {expected}, received {name}");
    }
    if trusted_id == Some(OFFICIAL_MARKETPLACE) && name != OFFICIAL_MARKETPLACE {
        bail!("Official marketplace source must provide {OFFICIAL_MARKETPLACE}, received {name}");
    }
    let mut plugin_count = loaded.plugin_count;
    let authority = known_path(storage);
    let target = storage.join("marketplaces").join(sanitize(name));
    let write_manifest = |staged: &Path| -> Result<()> {
        std::fs::write(
            staged.join("marketplace.json"),
            format!("{}\n", loaded.raw.pretty()),
        )?;
        Ok(())
    };
    let activation = if name == OFFICIAL_MARKETPLACE {
        // 官方市场：写 CDN 分片并与内置分片合并；记录数取合并后的目录。
        let merged = super::official_plugins_marketplace::write_cdn(storage, &loaded.raw)?;
        plugin_count = normalize(merged, true)?.plugin_count;
        None
    } else {
        Some(atomic_dir::activate(
            loaded.source_root.as_deref(),
            &target,
            &authority,
            write_manifest,
        )?)
    };
    let now = iso_now();
    let mut record = Json::object();
    record.set("id", Json::str(name));
    record.set("source", source.clone());
    record.set("name", Json::str(name));
    if let Some(description) = loaded.description.as_deref().filter(|d| !d.is_empty()) {
        record.set("description", Json::str(description));
    }
    record.set("addedAt", Json::str(now.clone()));
    record.set("lastUpdated", Json::str(now));
    record.set("pluginCount", Json::Number(plugin_count.into()));
    if let Some(activation) = &activation {
        record.set(
            "cacheTransactionId",
            Json::str(activation.transaction_id.clone()),
        );
    }
    // TS upsertKnownMarketplace：已有记录去掉旧 cacheTransactionId / lastRefreshFailure 后原地覆盖，保留 addedAt。
    let mut records = known(storage);
    let merged = match records.iter_mut().find(|r| id_of(r) == name) {
        Some(existing) => {
            let added = existing.get("addedAt").cloned();
            existing.remove("cacheTransactionId");
            existing.remove("lastRefreshFailure");
            if let Json::Object(fields) = &record {
                for (key, value) in fields {
                    existing.set(key, value.clone());
                }
            }
            if let Some(added) = added {
                existing.set("addedAt", added);
            }
            existing.clone()
        }
        None => {
            records.push(record.clone());
            record.clone()
        }
    };
    if let Err(error) = write_known(storage, records) {
        if let Some(activation) = activation {
            activation.rollback()?;
        }
        return Err(error);
    }
    if let Some(activation) = activation {
        activation.finalize();
    }
    Ok(merged)
}

/// TS `toValidationDiagnostic`（刷新失败落盘用的 code）。
pub(super) fn failure_code(error: &anyhow::Error) -> &'static str {
    if let Some(source) = error.downcast_ref::<plugin_git::SourceError>() {
        return source.code;
    }
    let message = error.to_string();
    if message.contains(UNSUPPORTED) {
        "plugin_marketplace_source_unsupported"
    } else if message.contains("Cross-marketplace dependency") {
        "plugin_dependency_cross_marketplace"
    } else if message.contains("dependency cycle") {
        "plugin_dependency_cycle"
    } else if message.contains("Dependency not found")
        || message.contains("Marketplace not found for dependency")
    {
        "plugin_dependency_missing"
    } else {
        "plugin_marketplace_invalid"
    }
}

/// TS `updateMarketplace`（单个已知市场）：用记录自带 source 受信任刷新；失败写 `lastRefreshFailure`。
pub(super) async fn refresh(
    storage: &Path,
    id: &str,
    cancel: &CancellationToken,
) -> Result<Option<Json>> {
    let Some(record) = known(storage).into_iter().find(|r| id_of(r) == id) else {
        bail!("Marketplace not found: {id}");
    };
    let source = record.get("source").cloned().unwrap_or_else(Json::object);
    match add(storage, &source, None, Some(id), cancel).await {
        Ok(updated) => Ok(Some(updated)),
        // 取消是本次操作的控制流，不是市场健康状态：不落 refresh failure（TS 同）。
        Err(error) if cancel.is_cancelled() => Err(error),
        Err(error) => {
            let mut records = known(storage);
            if let Some(existing) = records.iter_mut().find(|r| id_of(r) == id) {
                let mut failure = Json::object();
                failure.set("code", Json::str(failure_code(&error)));
                failure.set("failedAt", Json::str(iso_now()));
                failure.set("message", Json::str(error.to_string()));
                existing.set("lastRefreshFailure", failure);
                write_known(storage, records)?;
            }
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_input_forms() {
        let cwd = std::env::temp_dir();
        assert_eq!(
            parse_source_input("https://example.com/m.json", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"url","url":"https://example.com/m.json"}"#
        );
        assert_eq!(
            parse_source_input("https://github.com/a/b#main", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"git","url":"https://github.com/a/b.git","ref":"main"}"#
        );
        assert_eq!(
            parse_source_input("git@github.com:a/b.git", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"git","url":"git@github.com:a/b.git"}"#
        );
        assert_eq!(
            parse_source_input("acme/market@v1", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"github","repo":"acme/market","ref":"v1"}"#
        );
        assert!(parse_source_input("  ", &cwd).is_err());
        assert!(parse_source_input("nonsense", &cwd).is_err());
    }
}
