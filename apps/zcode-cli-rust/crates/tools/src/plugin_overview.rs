//! `plugins/overview`（docs/specs/rust-plugins.md 第 3 期）：对齐 TS `getPluginsOverview` →
//! `getZCodePluginsOverview`：市场摘要、目录条目（available）、已安装记录（含更新判定与 listing join）、
//! 可恢复的内置插件与诊断。存储解析见 `plugin_marketplace.rs`。

use super::plugin_marketplace::{self as market, NODE_REPL_HOST, OFFICIAL_MARKETPLACE};
use super::{extension_config as config, extension_plugins as plugins, plugin_list};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(super) async fn overview(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let config = plugin_list::config_for(&cwd, params).await?;
    let storage = config::storage(&config);
    market::ensure_default_marketplaces(&storage)?;
    let discovered = plugins::all(&cwd, &config, cancel).await?;
    let known = market::known_marketplaces(&storage);
    // Marketplace 声明只来自用户层（TS config-merger 会丢掉项目层的 extraKnownMarketplaces）。
    let declared = declared_marketplaces(&config::load_user().await?);

    let mut summaries = vec![];
    let mut available = vec![];
    let mut latest: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut listings: HashMap<String, Value> = HashMap::new();
    let installed = market::installed_records(&storage);
    let installed_ids: Vec<&str> = installed.iter().filter_map(|r| r["id"].as_str()).collect();
    for (record, use_cache) in effective_marketplaces(&known, &declared) {
        let id = record["id"].as_str().unwrap_or_default().to_owned();
        let manifest = use_cache.then(|| market::manifest(&storage, &id)).flatten();
        summaries.push(summary(&record, manifest.as_ref()));
        for entry in manifest.iter().flat_map(|m| &m.entries) {
            let plugin_id = format!("{}@{id}", entry.name);
            latest.insert(
                plugin_id.clone(),
                (entry.version.clone(), market::source_pin(&entry.source)),
            );
            if let Some(listing) = &entry.listing {
                listings.insert(plugin_id.clone(), listing.clone());
            }
            let mut item = json!({
                "id": plugin_id,
                "name": entry.name,
                "marketplace": id,
                "installed": installed_ids.contains(&plugin_id.as_str()),
                "componentTypes": market::component_types(&entry.raw),
            });
            if let Some(description) = &entry.description {
                item["description"] = description.as_str().into();
            }
            if let Some(version) = &entry.version {
                item["version"] = version.as_str().into();
            }
            if let Some(listing) = &entry.listing {
                item["listing"] = listing.clone();
            }
            available.push(item);
        }
    }

    let data_root = storage.join("data");
    let enabled_map = &config["plugins"]["enabledPlugins"];
    let mut installed_items = vec![];
    for record in &installed {
        let id = record["id"].as_str().unwrap_or_default();
        let loaded = discovered.iter().find(|plugin| plugin.id == id);
        let mut item = json!({
            "id": id,
            "name": record["name"],
            "marketplace": record["marketplace"],
            "enabled": enabled_map[id].as_bool().unwrap_or(false),
            "scope": record["scope"],
        });
        let mut version = record["version"].as_str().map(str::to_owned);
        if let Some(plugin) = loaded {
            if let Some(description) = plugin.manifest["description"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                item["description"] = description.into();
            }
            if let Some(manifest_version) = plugin.manifest["version"].as_str() {
                version = Some(manifest_version.to_owned());
            }
            let info = plugin_list::info(plugin, &cwd, &data_root).await?;
            item["componentTypes"] = component_types_from_info(&info);
        }
        if let Some(version) = version.as_deref().filter(|v| !v.is_empty()) {
            item["version"] = version.into();
        }
        for key in ["installPath", "installedAt"] {
            if record[key].as_str().is_some_and(|s| !s.is_empty()) {
                item[key] = record[key].clone();
            }
        }
        let (latest_version, latest_sha) = latest.get(id).cloned().unwrap_or_default();
        let installed_sha = market::source_pin(&record["source"]);
        item["updateStatus"] = market::update_status(
            version.as_deref(),
            installed_sha.as_deref(),
            latest_version.as_deref(),
            latest_sha.as_deref(),
        )
        .into();
        // TS：latestVersion 优先 manifest 的 version，否则用最新 sha 的前 7 位。
        let label = latest_version
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| latest_sha.as_ref().map(|sha| sha.chars().take(7).collect()));
        if let Some(label) = label.filter(|l| !l.is_empty()) {
            item["latestVersion"] = label.into();
        }
        if let Some(listing) = listings.get(id) {
            item["listing"] = listing.clone();
        }
        installed_items.push(item);
    }

    let mut diagnostics = declaration_diagnostics(&known, &declared);
    for record in &known {
        let failure = &record["lastRefreshFailure"];
        if failure.is_object() {
            diagnostics.push(json!({
                "code": failure["code"],
                "message": failure["message"],
                "severity": "error",
                "pluginId": record["id"],
            }));
        }
    }
    Ok(json!({
        "marketplaces": summaries,
        "availablePlugins": available,
        "installedPlugins": installed_items,
        "restorableBuiltins": restorable_builtins(&config),
        "diagnostics": diagnostics,
        "capability": { "supported": true },
    }))
}

/// TS `toMarketplaceSummaryData` + `countVisibleMarketplacePlugins`。
fn summary(record: &Value, manifest: Option<&market::Manifest>) -> Value {
    let id = record["id"].as_str().unwrap_or_default();
    let visible = manifest.map(|m| {
        m.entries
            .iter()
            .filter(|e| id != OFFICIAL_MARKETPLACE || e.name != NODE_REPL_HOST)
            .count()
    });
    let mut item = json!({
        "id": id,
        "name": record["name"],
        "source": record["source"],
        "pluginCount": visible.map_or_else(|| record["pluginCount"].clone(), Value::from),
        "isOfficial": id == OFFICIAL_MARKETPLACE,
    });
    for key in ["description", "lastUpdated"] {
        if record[key].as_str().is_some_and(|s| !s.is_empty()) {
            item[key] = record[key].clone();
        }
    }
    let failure = &record["lastRefreshFailure"];
    if failure.is_object() {
        item["refreshFailure"] = json!({
            "code": failure["code"],
            "failedAt": failure["failedAt"],
            "message": failure["message"],
        });
    }
    if let Some(manifest) = manifest.filter(|m| !m.featured.is_empty()) {
        item["featured"] = manifest.featured.clone().into();
    }
    item
}

/// TS `resolveDeclaredMarketplaceSources`：`plugins.extraKnownMarketplaces`，file/directory 源的相对路径按
/// 用户配置所在目录（`~/.zcode/cli`）解析。
fn declared_marketplaces(user_config: &Value) -> Vec<(String, Value)> {
    let base = config::home().join(".zcode").join("cli");
    let Some(map) = user_config["plugins"]["extraKnownMarketplaces"].as_object() else {
        return vec![];
    };
    map.iter()
        .filter(|(id, declaration)| !id.is_empty() && declaration["source"].is_object())
        .map(|(id, declaration)| {
            let mut source = declaration["source"].clone();
            let kind = source["source"].as_str().unwrap_or_default();
            if (kind == "file" || kind == "directory")
                && let Some(path) = source["path"].as_str()
            {
                let path = PathBuf::from(path);
                let resolved = if path.is_absolute() {
                    path
                } else {
                    base.join(path)
                };
                source["path"] = super::lexical_path::normalize(&resolved)
                    .to_string_lossy()
                    .into_owned()
                    .into();
            }
            (id.clone(), source)
        })
        .collect()
}

/// TS `resolveEffectiveMarketplaceRecords`：同 id 同 source 读缓存；异 source 的非官方声明替换成
/// 不读缓存的占位记录；官方 id 是保留身份，声明只产生诊断。
fn effective_marketplaces(known: &[Value], declared: &[(String, Value)]) -> Vec<(Value, bool)> {
    let mut records: Vec<(Value, bool)> = known
        .iter()
        .map(|record| {
            let id = record["id"].as_str().unwrap_or_default();
            match declared.iter().find(|(declared_id, _)| declared_id == id) {
                Some((_, source)) if *source != record["source"] && id != OFFICIAL_MARKETPLACE => {
                    (declared_record(id, source), false)
                }
                _ => (record.clone(), true),
            }
        })
        .collect();
    for (id, source) in declared {
        let is_known = known.iter().any(|record| record["id"] == id.as_str());
        if !is_known && id != OFFICIAL_MARKETPLACE {
            records.push((declared_record(id, source), false));
        }
    }
    records
}

fn declared_record(id: &str, source: &Value) -> Value {
    json!({ "id": id, "source": source, "name": id, "addedAt": "", "pluginCount": 0 })
}

/// TS `resolveMarketplaceDeclarationDiagnostics`。
fn declaration_diagnostics(known: &[Value], declared: &[(String, Value)]) -> Vec<Value> {
    declared
        .iter()
        .filter(|(id, source)| {
            id == OFFICIAL_MARKETPLACE
                && !known
                    .iter()
                    .any(|record| record["id"] == id.as_str() && record["source"] == *source)
        })
        .map(|(id, _)| {
            json!({
                "code": "plugin_marketplace_declaration_reserved",
                "message": format!(
                    "Workspace marketplace declaration \"{id}\" uses a reserved official id and was ignored. \
                     Use a different marketplace id for project declarations."
                ),
                "severity": "warning",
                "pluginId": id,
            })
        })
        .collect()
}

/// TS `inferComponentTypesFromMetadata`（hook 依赖 hookDetails，留到第 4 期）。
fn component_types_from_info(info: &Value) -> Value {
    let mut types: Vec<&str> = vec![];
    let has_agents = info["components"].as_array().is_some_and(|groups| {
        groups.iter().any(|g| {
            g["kind"] == "agent" && g["items"].as_array().is_some_and(|items| !items.is_empty())
        })
    });
    let count = |key: &str| info[key].as_u64().unwrap_or(0);
    let non_empty = |key: &str| info[key].as_array().is_some_and(|a| !a.is_empty());
    if has_agents {
        types.push("agent");
    }
    if count("commandRootCount") > 0 {
        types.push("command");
    }
    if count("skillRootCount") > 0 || count("skillCount") > 0 {
        types.push("skill");
    }
    if non_empty("declaredMcpServerNames") || non_empty("mcpServerNames") {
        types.push("mcp");
    }
    json!(types)
}

/// TS overview 的 `restorableBuiltins`：被 `suppressedBuiltins` 抑制的官方插件定义（computer-use 另需
/// CUA 特性开启），商店信息取定义里的 listing seed。
fn restorable_builtins(config: &Value) -> Vec<Value> {
    let suppressed = config::strings(&config["plugins"]["suppressedBuiltins"]);
    super::official_plugins::assets()
        .definitions
        .iter()
        .filter(|definition| {
            suppressed.contains(&format!("{}@{OFFICIAL_MARKETPLACE}", definition.name).as_str())
                && (definition.name != "computer-use" || cua_feature_enabled())
        })
        .map(|definition| {
            let mut item = json!({
                "id": format!("{}@{OFFICIAL_MARKETPLACE}", definition.name),
                "name": definition.name,
                "marketplace": OFFICIAL_MARKETPLACE,
                "installed": false,
            });
            if !definition.version.is_empty() {
                item["version"] = definition.version.as_str().into();
            }
            let seed = definition
                .listing
                .as_ref()
                .and_then(|listing| serde_json::from_str::<Value>(&listing.compact()).ok());
            if let Some(Value::Object(seed)) = seed {
                let mut entry = Map::new();
                entry.insert("name".into(), definition.name.clone().into());
                entry.extend(seed);
                if let Some(listing) = market::listing(&entry) {
                    item["listing"] = listing;
                }
            }
            item
        })
        .collect()
}

/// TS `isZCodeCuaInternalFeatureEnabled`：DEV_MODE 打开即启用，PRODUCT_HELPER=0/false/off 显式关闭。
fn cua_feature_enabled() -> bool {
    let env = |key: &str| std::env::var(key).unwrap_or_default().trim().to_lowercase();
    if matches!(env("ZCODE_CUA_DEV_MODE").as_str(), "1" | "true" | "on") {
        return true;
    }
    !matches!(
        env("ZCODE_CUA_PRODUCT_HELPER").as_str(),
        "0" | "false" | "off"
    )
}
