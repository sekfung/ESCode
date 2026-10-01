//! `plugins/validate`（docs/specs/rust-plugin-marketplace-write.md W5b）：对齐 TS `validatePlugin` →
//! `validateZCodePlugin` → adapters `validateMarketplaceSource` / `validateMarketplacePlugin` /
//! `validatePluginRoot`。只读：远端源物化到临时目录后即删除，不写存储。

use super::plugin_install as install;
use super::plugin_marketplace as market;
use super::plugin_uninstall::storage_lock;
use super::plugin_validate_shape::{
    compatibility, deferral, entry_manifest, entry_shape, validate_root,
};
use super::{extension_config as config, plugin_list, plugin_market_write as market_write};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) async fn validate(params: &Value) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let field = |key: &str| plugin_list::non_empty(params, key).ok().map(str::to_owned);
    let (name, marketplace, source) = (field("pluginName"), field("marketplace"), field("source"));
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    market::ensure_default_marketplaces(&storage)?;
    let diagnostics = if let Some(source) = source {
        match market_write::parse_source_input(&source, &cwd) {
            Ok(source) => validate_source(&storage, &source, None, None).await,
            Err(error) => vec![diag(
                "plugin_marketplace_invalid",
                error.to_string(),
                None,
                true,
            )],
        }
    } else if let (Some(name), Some(marketplace)) = (name, marketplace) {
        let id = format!("{name}@{marketplace}");
        match market_write::ensure_manifest(&storage, &marketplace).await {
            Err(error) => vec![diag(
                "plugin_marketplace_invalid",
                error.to_string(),
                Some(&id),
                true,
            )],
            Ok(()) => {
                tokio::task::spawn_blocking(move || validate_plugin(&storage, &marketplace, &name))
                    .await
                    .map_err(|_| anyhow!("Plugin validate worker panicked"))?
            }
        }
    } else {
        vec![]
    };
    let ok = diagnostics.iter().all(|d| d["severity"] != "error");
    Ok(json!({
        "ok": ok,
        "diagnostics": diagnostics,
        "compatibility": {
            "runnable": ["skills", "commands", "hooks", "mcpServers", "userConfig"],
            "diagnosticOnly": ["agents", "lspServers", "outputStyles", "channels", "settings"],
            "unsupported": ["mcpb", "dxt", "npm", "hostPattern", "pathPattern"],
        },
    }))
}

pub(super) fn diag(
    code: &str,
    message: impl Into<String>,
    plugin_id: Option<&str>,
    error: bool,
) -> Value {
    let mut out = json!({
        "code": code,
        "message": message.into(),
        "severity": if error { "error" } else { "warning" },
    });
    if let Some(id) = plugin_id {
        out["pluginId"] = id.into();
    }
    out
}

/// TS `toValidationDiagnostic`：源错误带 code；其余按文案归类。
pub(super) fn error_diagnostic(error: &anyhow::Error, plugin_id: Option<&str>) -> Value {
    let message = error.to_string();
    let code = if let Some(source) = error.downcast_ref::<super::plugin_git::SourceError>() {
        source.code
    } else if message.contains("source is recognized but not supported in this runtime") {
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
    };
    diag(code, message, plugin_id, true)
}

pub(super) fn entry_name(entry: &Json) -> String {
    entry
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// TS `installZCodeMarketplacePlugin` 的 dryRun 分支：声明源与已知记录冲突 → 改指诊断；声明了但未落盘 →
/// 按声明源校验（不落盘）；否则按已知市场校验单个插件。
pub(super) async fn install_dry_run(
    storage: &Path,
    marketplace: &str,
    name: &str,
) -> Result<Value> {
    let user = config::load_user().await?;
    let declared = super::plugin_overview::declared_marketplaces(&user)
        .into_iter()
        .find(|(id, _)| id == marketplace)
        .map(|(_, source)| source);
    let known = market_write::known(storage)
        .into_iter()
        .find(|record| record.get("id").and_then(Json::as_str) == Some(marketplace));
    let known_source = known.as_ref().map(|record| {
        record
            .get("source")
            .and_then(|s| serde_json::from_str::<Value>(&s.compact()).ok())
            .unwrap_or(Value::Null)
    });
    let diagnostics = match (&declared, &known_source) {
        (Some(declared), Some(known)) if declared != known => vec![diag(
            "plugin_marketplace_invalid",
            format!(
                "Workspace marketplace declaration \"{marketplace}\" conflicts with an existing Host source. Remove the existing marketplace or use a different marketplace id before materializing it."
            ),
            Some(marketplace),
            true,
        )],
        (Some(declared), _)
            if known.is_none() || market::manifest(storage, marketplace).is_none() =>
        {
            let source = Json::parse(&declared.to_string()).unwrap_or_else(Json::object);
            validate_source(storage, &source, Some(marketplace), Some(name)).await
        }
        _ => {
            let id = format!("{name}@{marketplace}");
            match market_write::ensure_manifest(storage, marketplace).await {
                Err(error) => vec![error_diagnostic(&error, Some(&id))],
                Ok(()) => {
                    let (storage, marketplace, name) =
                        (storage.to_owned(), marketplace.to_owned(), name.to_owned());
                    tokio::task::spawn_blocking(move || {
                        validate_plugin(&storage, &marketplace, &name)
                    })
                    .await
                    .map_err(|_| anyhow!("Plugin validate worker panicked"))?
                }
            }
        }
    };
    Ok(json!({ "dependencyClosure": [], "installedPlugins": [], "diagnostics": diagnostics }))
}

/// TS `validateMarketplacePlugin`（阻塞线程）。
fn validate_plugin(storage: &Path, marketplace: &str, name: &str) -> Vec<Value> {
    let id = format!("{name}@{marketplace}");
    if market::manifest(storage, marketplace).is_none() {
        return vec![diag(
            "plugin_marketplace_invalid",
            format!("Marketplace not found: {marketplace}"),
            None,
            true,
        )];
    }
    let Some(entry) = install::ordered_entry(storage, marketplace, name) else {
        return vec![diag(
            "plugin_not_found",
            format!("Plugin not found: {id}"),
            None,
            true,
        )];
    };
    let mut diagnostics = vec![];
    if let Err(error) = install::closure_in(storage, marketplace, name, None) {
        diagnostics.push(error_diagnostic(&error, Some(&id)));
    }
    match install::materialize(storage, marketplace, &entry, None) {
        Ok(root) => {
            diagnostics.extend(validate_root(&entry, marketplace, &root.path));
            root.cleanup();
        }
        Err(error) => diagnostics.push(error_diagnostic(&error, Some(&id))),
    }
    diagnostics
}

/// TS `validateMarketplaceSource`（persist:false）：加载市场，逐条目校验形状 / 依赖；远端条目延后深扫。
/// `expected`：声明的市场 id（不一致即报错）；`plugin`：只校验该条目。
async fn validate_source(
    storage: &Path,
    source: &Json,
    expected: Option<&str>,
    plugin: Option<&str>,
) -> Vec<Value> {
    let loaded = match market_write::load(source).await {
        Ok(loaded) => loaded,
        Err(error) => return vec![error_diagnostic(&error, None)],
    };
    if let Some(expected) = expected.filter(|e| *e != loaded.name) {
        if let Some(temp) = &loaded.temp {
            let _ = std::fs::remove_dir_all(temp);
        }
        let message = format!(
            "Marketplace declaration id mismatch: expected {expected}, received {}",
            loaded.name
        );
        return vec![diag(
            "plugin_marketplace_invalid",
            message,
            Some(expected),
            true,
        )];
    }
    let storage = storage.to_owned();
    let plugin = plugin.map(str::to_owned);
    let raw = loaded.raw.clone();
    let name = loaded.name.clone();
    let source_root = loaded.source_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        validate_loaded(
            &storage,
            &raw,
            &name,
            source_root.as_deref(),
            plugin.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|_| {
        vec![diag(
            "plugin_marketplace_invalid",
            "Plugin validate worker panicked",
            None,
            true,
        )]
    });
    if let Some(temp) = &loaded.temp {
        let _ = std::fs::remove_dir_all(temp);
    }
    result
}

fn validate_loaded(
    storage: &Path,
    raw: &Json,
    marketplace: &str,
    source_root: Option<&Path>,
    plugin: Option<&str>,
) -> Vec<Value> {
    let mut diagnostics = vec![];
    let entries: Vec<&Json> = raw
        .get("plugins")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry.is_object() && !entry_name(entry).is_empty())
        .collect();
    if entries.is_empty() {
        diagnostics.push(diag(
            "plugin_marketplace_invalid",
            format!("Marketplace has no plugins: {marketplace}"),
            None,
            false,
        ));
    }
    let entries: Vec<&Json> = match plugin {
        Some(plugin) => entries
            .into_iter()
            .filter(|entry| entry_name(entry) == plugin)
            .collect(),
        None => entries,
    };
    if let Some(plugin) = plugin.filter(|_| entries.is_empty()) {
        let id = format!("{plugin}@{marketplace}");
        diagnostics.push(diag(
            "plugin_not_found",
            format!("Plugin not found: {id}"),
            Some(&id),
            true,
        ));
        return diagnostics;
    }
    // 相对源按市场源目录解析（TS sourceRoot ?? 存储里的市场目录）。
    let dir: PathBuf = source_root.map(Path::to_owned).unwrap_or_else(|| {
        storage
            .join("marketplaces")
            .join(market::sanitize(marketplace))
    });
    for entry in entries {
        let name = entry_name(entry);
        let id = format!("{name}@{marketplace}");
        diagnostics.extend(entry_shape(entry, &id));
        if let Err(error) = install::closure_in(storage, marketplace, &name, Some(raw)) {
            diagnostics.push(error_diagnostic(&error, Some(&id)));
        }
        if let Some(deferred) = deferral(entry, &id) {
            diagnostics.push(deferred);
            diagnostics.extend(compatibility(&entry_manifest(entry), &id));
            continue;
        }
        match install::materialize(storage, marketplace, entry, Some((&dir, raw))) {
            Ok(root) => {
                diagnostics.extend(validate_root(entry, marketplace, &root.path));
                root.cleanup();
            }
            Err(error) => {
                diagnostics.push(error_diagnostic(&error, Some(&id)));
                // 源暂时不可解析时仍基于条目原文给出 diagnostic-only 能力诊断。
                diagnostics.extend(compatibility(&entry_manifest(entry), &id));
            }
        }
    }
    diagnostics
}
