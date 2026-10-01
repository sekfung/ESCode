//! 插件市场的只读存储面（docs/specs/rust-plugins.md 第 3 期），对齐 TS
//! `adapters/src/plugins/marketplace.ts`：`known_marketplaces.json`、`marketplaces/<id>/marketplace.json`、
//! `installed_plugins.json` 的解析与归一化，目录条目的商店信息（listing），以及版本更新判定。
//!
//! 写面只有一处：`ensure_default_marketplaces`（TS `ensureDefaultPluginMarketplaces`，overview 每次都会
//! 补齐缺失的官方市场记录）；安装/刷新/移除仍由 Node 侧负责（见 spec「市场写面不在本分期」）。

use crate::domain::json_order::Json;
use serde_json::{Map, Value, json};
use std::path::Path;

#[allow(unused_imports)]
pub(super) use super::plugin_listing::{author, component_types, listing, source_pin};

pub(super) const OFFICIAL_MARKETPLACE: &str = "zcode-plugins-official";

/// TS `OFFICIAL_NODE_REPL_HOST_PLUGIN_NAME`：官方市场里的宿主插件，不计入可见数量。
pub(super) const NODE_REPL_HOST: &str = "node-repl-host";

const DEFAULT_OFFICIAL_SOURCE: &str =
    "https://cdn-zcode.z.ai/zcode/official-plugin/marketplace.json";

const DEFAULT_OFFICIAL_DESCRIPTION: &str =
    "Official ZCode plugins marketplace: built-in and community plugins for ZCode.";

/// TS `readJsonFileSync`：读不到或不是 JSON 一律视为缺失。
/// 未实现 `recoverAtomicTargetSync` 的崩溃恢复（Rust 不写这些目录，见 spec 第 3 期说明）。
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// TS `isKnownMarketplaceRecord`。
fn is_known_record(value: &Value) -> bool {
    value["id"].is_string()
        && value["name"].is_string()
        && value["pluginCount"].is_number()
        && value["source"].is_object()
}

/// TS `loadKnownMarketplacesSync`：`marketplaces` 可以是数组或 id → record 的对象。
pub(super) fn known_marketplaces(storage: &Path) -> Vec<Value> {
    let Some(parsed) = read_json(&storage.join("known_marketplaces.json")) else {
        return vec![];
    };
    let records: Vec<Value> = match &parsed["marketplaces"] {
        Value::Array(items) => items.clone(),
        Value::Object(map) => map.values().cloned().collect(),
        _ => vec![],
    };
    records.into_iter().filter(is_known_record).collect()
}

/// TS `ensureDefaultPluginMarketplaces`：缺官方市场记录时补一条并整份重写
/// `{version: 1, marketplaces: [...]}`（保序：已有记录原样、未通过校验的记录被丢弃，与 TS 相同）。
pub(super) fn ensure_default_marketplaces(storage: &Path) -> std::io::Result<()> {
    let path = storage.join("known_marketplaces.json");
    let parsed = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| Json::parse(&text))
        .filter(Json::is_object);
    let mut records: Vec<Json> = match parsed.as_ref().and_then(|p| p.get("marketplaces")) {
        Some(Json::Array(items)) => items.clone(),
        Some(Json::Object(entries)) => entries.iter().map(|(_, v)| v.clone()).collect(),
        _ => vec![],
    };
    records.retain(|record| {
        serde_json::from_str::<Value>(&record.compact()).is_ok_and(|v| is_known_record(&v))
    });
    let has_official = records
        .iter()
        .any(|record| record.get("id").and_then(Json::as_str) == Some(OFFICIAL_MARKETPLACE));
    if has_official {
        return Ok(());
    }
    let mut source = Json::object();
    source.set("source", Json::str("url"));
    source.set("url", Json::str(DEFAULT_OFFICIAL_SOURCE));
    let mut record = Json::object();
    record.set("id", Json::str(OFFICIAL_MARKETPLACE));
    record.set("source", source);
    record.set("name", Json::str(OFFICIAL_MARKETPLACE));
    record.set("description", Json::str(DEFAULT_OFFICIAL_DESCRIPTION));
    record.set("addedAt", Json::str(iso_now()));
    record.set("pluginCount", Json::Number(0.into()));
    records.push(record);
    let mut file = Json::object();
    file.set("version", Json::Number(1.into()));
    file.set("marketplaces", Json::Array(records));
    std::fs::create_dir_all(storage)?;
    std::fs::write(&path, format!("{}\n", file.pretty()))
}

/// JS `new Date().toISOString()`（毫秒精度、UTC、`Z` 结尾）。
fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// TS `sanitizePluginId`。
pub(super) fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.@-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// TS `PluginMarketplaceManifest` 中 overview 用到的部分。
pub(super) struct Manifest {
    pub featured: Vec<String>,
    pub entries: Vec<Entry>,
}

pub(super) struct Entry {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub source: Value,
    pub listing: Option<Value>,
    /// 条目原样（object 形式的 `plugins` 会先补上 `name`），供 componentTypes 推断。
    pub raw: Map<String, Value>,
}

/// TS `loadMarketplaceManifestSync` + `parseMarketplaceManifest` + `normalizeMarketplaceManifest`。
pub(super) fn manifest(storage: &Path, marketplace: &str) -> Option<Manifest> {
    let parsed = read_json(
        &storage
            .join("marketplaces")
            .join(sanitize(marketplace))
            .join("marketplace.json"),
    )?;
    let name = parsed["name"].as_str().unwrap_or_default().trim();
    if !valid_marketplace_name(name) {
        return None;
    }
    let raw_entries: Vec<Map<String, Value>> = match &parsed["plugins"] {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_object().cloned())
            .collect(),
        Value::Object(map) => map
            .iter()
            .map(|(name, plugin)| {
                let mut entry = Map::new();
                entry.insert("name".into(), name.clone().into());
                if let Some(fields) = plugin.as_object() {
                    for (key, value) in fields {
                        entry.insert(key.clone(), value.clone());
                    }
                }
                entry
            })
            .collect(),
        _ => vec![],
    };
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let entries = raw_entries
        .into_iter()
        .filter_map(|raw| {
            let name = raw.get("name")?.as_str()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }
            let value = Value::Object(raw.clone());
            Some(Entry {
                name,
                description: text(&value["description"]),
                version: text(&value["version"]),
                source: value["source"].clone(),
                listing: listing(&raw),
                raw,
            })
        })
        .collect();
    let featured = parsed["featured"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(Manifest { featured, entries })
}

/// TS `MARKETPLACE_NAME_PATTERN = /^[a-z0-9][a-z0-9._-]{0,127}$/`。
fn valid_marketplace_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 128
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
}

/// TS `listInstalledPluginRecords`（`normalizeInstalledPluginsState`）：数组形式逐条校验；
/// 对象形式（Claude 风格 id → entry | entry[]）缺 installPath 的条目丢弃，scope project/local → workspace。
pub(super) fn installed_records(storage: &Path) -> Vec<Value> {
    let Some(parsed) = read_json(&storage.join("installed_plugins.json")) else {
        return vec![];
    };
    match &parsed["plugins"] {
        Value::Object(map) => map
            .iter()
            .flat_map(|(id, entry)| {
                let entries = match entry {
                    Value::Array(items) => items.clone(),
                    other => vec![other.clone()],
                };
                let id = id.clone();
                entries.into_iter().filter_map(move |item| {
                    let item = item.as_object()?;
                    let install_path = item.get("installPath")?.as_str()?.to_owned();
                    let (name, marketplace) = id.rsplit_once('@')?;
                    if install_path.is_empty() || name.is_empty() || marketplace.is_empty() {
                        return None;
                    }
                    let text = |key: &str| item.get(key).and_then(Value::as_str);
                    let scope = match text("scope") {
                        Some("project" | "local") => "workspace",
                        _ => "user",
                    };
                    Some(json!({
                        "id": id,
                        "name": name,
                        "marketplace": marketplace,
                        "version": text("version").unwrap_or("0.0.0"),
                        "installPath": install_path,
                        "installedAt": text("installedAt").unwrap_or("1970-01-01T00:00:00.000Z"),
                        "scope": scope,
                    }))
                })
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .filter(|v| {
                [
                    "id",
                    "name",
                    "marketplace",
                    "version",
                    "installPath",
                    "installedAt",
                ]
                .iter()
                .all(|key| v[key].is_string())
                    && (v["scope"] == "user" || v["scope"] == "workspace")
            })
            .cloned()
            .collect(),
        _ => vec![],
    }
}

/// TS `comparePluginUpdate` + `comparePluginVersions`（semver `coerce` 后比较；无法解析时按字符串是否相同）。
pub(super) fn update_status(
    installed_version: Option<&str>,
    installed_sha: Option<&str>,
    latest_version: Option<&str>,
    latest_sha: Option<&str>,
) -> &'static str {
    if let Some(latest) = latest_version.filter(|v| !v.is_empty()) {
        let Some(installed) = installed_version.filter(|v| !v.is_empty()) else {
            return "none";
        };
        return match (coerce(installed), coerce(latest)) {
            (Some(installed), Some(latest)) if latest > installed => "update-available",
            (Some(_), Some(_)) => "none",
            _ if installed == latest => "none",
            _ => "version-changed",
        };
    }
    if let Some(latest) = latest_sha.filter(|v| !v.is_empty()) {
        return match installed_sha.filter(|v| !v.is_empty()) {
            None => "version-changed",
            Some(installed) if installed == latest => "none",
            Some(_) => "update-available",
        };
    }
    "none"
}

/// node-semver `coerce`（非 loose / 非 rtl）：取第一段 `\d{1,16}(\.\d{1,16}){0,2}`，缺省位补 0。
fn coerce(version: &str) -> Option<(u64, u64, u64)> {
    let bytes = version.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() || (i > 0 && bytes[i - 1].is_ascii_digit()) {
            i += 1;
            continue;
        }
        let run = |start: usize| {
            let end = bytes[start..]
                .iter()
                .position(|b| !b.is_ascii_digit())
                .map_or(bytes.len(), |n| start + n);
            (end, &version[start..end])
        };
        let (mut end, major) = run(i);
        if major.len() > 16 {
            i = end;
            continue;
        }
        let mut parts = vec![major.parse::<u64>().ok()?];
        while parts.len() < 3 && bytes.get(end) == Some(&b'.') {
            let (next_end, digits) = run(end + 1);
            if digits.is_empty() || digits.len() > 16 {
                break;
            }
            parts.push(digits.parse().ok()?);
            end = next_end;
        }
        parts.resize(3, 0);
        return Some((parts[0], parts[1], parts[2]));
    }
    None
}

#[cfg(test)]
#[path = "plugin_marketplace_tests.rs"]
mod tests;
