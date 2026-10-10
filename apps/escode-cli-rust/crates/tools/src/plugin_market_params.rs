//! `plugins/marketplace/add|remove|update` 的协议入口与按需拉取（TS escode-protocol/plugins.ts 对应方法）。
#[allow(unused_imports)]
use super::plugin_market_write::*;

use super::plugin_git;
use super::plugin_marketplace::OFFICIAL_MARKETPLACE;
use crate::domain::json_order::Json;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

/// TS `toMarketplaceSummaryData(record)`（无 featured / 可见数投影）。
pub(super) fn summary(record: &Json) -> Value {
    let record: Value = serde_json::from_str(&record.compact()).unwrap_or_default();
    let id = record["id"].as_str().unwrap_or_default();
    let mut out = json!({
        "id": id,
        "name": record["name"],
        "source": record["source"],
        "pluginCount": record["pluginCount"],
        "isOfficial": id == OFFICIAL_MARKETPLACE,
    });
    for key in ["description", "lastUpdated"] {
        if record[key].as_str().is_some_and(|s| !s.is_empty()) {
            out[key] = record[key].clone();
        }
    }
    let failure = &record["lastRefreshFailure"];
    if failure.is_object() {
        out["refreshFailure"] = json!({
            "code": failure["code"],
            "failedAt": failure["failedAt"],
            "message": failure["message"],
        });
    }
    out
}

// ---- 协议入口 ----

pub(super) async fn context(params: &Value) -> Result<(PathBuf, PathBuf, Value)> {
    let cwd = super::plugin_list::workspace_path(params)?;
    let config = super::extension_config::load(&cwd).await?;
    let storage = super::extension_config::storage(&config);
    Ok((cwd, storage, config))
}

pub(super) async fn add_params(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let source_input = super::plugin_list::non_empty(params, "source")?.to_owned();
    let (cwd, storage, _) = context(params).await?;
    let _guard = super::plugin_uninstall::storage_lock(&storage).await;
    let source = parse_source_input(&source_input, &cwd)?;
    if params["dryRun"] == true {
        let source: Value = serde_json::from_str(&source.compact())?;
        return Ok(json!({
            "marketplace": {"id": "dry-run", "name": "dry-run", "source": source, "pluginCount": 0, "isOfficial": false},
            "diagnostics": [],
        }));
    }
    let record = add(&storage, &source, None, None, cancel).await?;
    Ok(json!({ "marketplace": summary(&record), "diagnostics": [] }))
}

pub(super) async fn remove_params(params: &Value) -> Result<Value> {
    let id = super::plugin_list::non_empty(params, "marketplace")?.to_owned();
    let (_, storage, _) = context(params).await?;
    let _guard = super::plugin_uninstall::storage_lock(&storage).await;
    let records: Vec<Json> = known(&storage)
        .into_iter()
        .filter(|r| id_of(r) != id)
        .collect();
    write_known(&storage, records)?;
    Ok(json!({ "diagnostics": [] }))
}

/// TS `updateESCodePluginMarketplace`：指定 id 时只刷新它（声明未物化则按声明 add）；否则刷新全部已知市场。
pub(super) async fn update_params(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let only = params["marketplace"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let (_, storage, _) = context(params).await?;
    let _guard = super::plugin_uninstall::storage_lock(&storage).await;
    super::plugin_marketplace::ensure_default_marketplaces(&storage)?;
    let user = super::extension_config::load_user().await?;
    let declared: Vec<(String, Json)> = user["plugins"]["extraKnownMarketplaces"]
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .filter(|(_, d)| d["source"].is_object())
                .filter_map(|(id, d)| Some((id.clone(), Json::parse(&d["source"].to_string())?)))
                .collect()
        })
        .unwrap_or_default();
    let known_ids: Vec<String> = known(&storage)
        .iter()
        .map(|r| id_of(r).to_owned())
        .collect();
    if let Some(id) = &only
        && !known_ids.contains(id)
        && !declared.iter().any(|(d, _)| d == id)
    {
        bail!("Marketplace not found: {id}");
    }
    let targets: Vec<String> = match &only {
        Some(id) => vec![id.clone()],
        None => known_ids.clone(),
    };
    let mut updated = vec![];
    let mut diagnostics = vec![];
    for id in targets {
        let declared_source = declared.iter().find(|(d, _)| d == &id).map(|(_, s)| s);
        let known_record = known(&storage).into_iter().find(|r| id_of(r) == id);
        if only.is_some()
            && let (Some(declared), Some(record)) = (declared_source, &known_record)
            && record.get("source").map(Json::compact) != Some(declared.compact())
        {
            diagnostics.push(json!({
                "code": "plugin_marketplace_invalid",
                "message": format!("Workspace marketplace declaration \"{id}\" conflicts with an existing Host source. Remove the existing marketplace or use a different marketplace id before materializing it."),
                "severity": "error",
                "pluginId": id,
            }));
            continue;
        }
        if let (Some(declared), None) = (declared_source, &known_record) {
            match add(&storage, declared, Some(&id), None, cancel).await {
                Ok(record) => updated.push(record),
                Err(error) => diagnostics.push(json!({
                    "code": error.downcast_ref::<plugin_git::SourceError>().map_or("plugin_marketplace_invalid", |e| e.code),
                    "message": error.to_string(),
                    "severity": "error",
                    "pluginId": id,
                })),
            }
            continue;
        }
        if let Some(record) = refresh(&storage, &id, cancel).await? {
            updated.push(record);
        }
    }
    for record in known(&storage) {
        let id = id_of(&record);
        if only.as_deref().is_some_and(|o| o != id) {
            continue;
        }
        if let Some(failure) = record.get("lastRefreshFailure").filter(|f| f.is_object()) {
            diagnostics.push(json!({
                "code": failure.get("code").and_then(Json::as_str),
                "message": failure.get("message").and_then(Json::as_str),
                "severity": "error",
                "pluginId": id,
            }));
        }
    }
    Ok(json!({
        "marketplaces": updated.iter().map(summary).collect::<Vec<_>>(),
        "diagnostics": diagnostics,
    }))
}

/// TS `ensureMarketplaceManifestAvailable`：本地没有 manifest 但有已知记录时，用记录 source 受信任拉取。
pub(super) async fn ensure_manifest(storage: &Path, marketplace: &str) -> Result<()> {
    if super::plugin_marketplace::manifest(storage, marketplace).is_some() {
        return Ok(());
    }
    let Some(record) = known(storage).into_iter().find(|r| id_of(r) == marketplace) else {
        return Ok(());
    };
    let source = record.get("source").cloned().unwrap_or_else(Json::object);
    add(
        storage,
        &source,
        None,
        Some(marketplace),
        &CancellationToken::new(),
    )
    .await
    .map(|_| ())
}
