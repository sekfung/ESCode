//! 安装用的市场条目读取、依赖闭包与路径工具（TS marketplace.ts 的 entry / dependency / path 辅助）。
#[allow(unused_imports)]
use super::plugin_install::*;

use super::atomic_dir;
use super::plugin_marketplace::{self as market, sanitize};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 市场 manifest 原文（保序）。TS 用解析后的条目，但合成 plugin.json 与 installed record 的 `source`
/// 都要按条目原样的 key 顺序输出。
pub(super) fn ordered_manifest(storage: &Path, marketplace: &str) -> Option<Json> {
    let path = storage
        .join("marketplaces")
        .join(sanitize(marketplace))
        .join("marketplace.json");
    let text = std::fs::read_to_string(atomic_dir::recover(&path)).ok()?;
    Json::parse(&text).filter(Json::is_object)
}

/// 条目原文：`plugins` 为数组时按 trim 后的 name 匹配；为对象时 key 即 name（补上 `name`）。
pub(super) fn ordered_entry(storage: &Path, marketplace: &str, name: &str) -> Option<Json> {
    entry_in(&ordered_manifest(storage, marketplace)?, name)
}

/// 在给定的市场 manifest 原文里按名字取条目（见 [`ordered_entry`]）。
pub(super) fn entry_in(manifest: &Json, name: &str) -> Option<Json> {
    match manifest.get("plugins")? {
        Json::Array(items) => items
            .iter()
            .find(|item| item.get("name").and_then(Json::as_str).map(str::trim) == Some(name))
            .cloned(),
        Json::Object(entries) => entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(key, value)| {
                let mut entry = Json::object();
                entry.set("name", Json::str(key));
                if let Json::Object(fields) = value {
                    for (k, v) in fields {
                        entry.set(k, v.clone());
                    }
                }
                entry
            }),
        _ => None,
    }
}

/// TS `normalizeDependencyRef`：installed record 的 `dependencies` 记录的就是这一份（未限定市场）。
pub(super) fn normalized_dependencies(entry: &Json) -> Vec<String> {
    let Some(items) = entry.get("dependencies").and_then(Json::as_array) else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| match item {
            Json::String(text) => {
                // `name@^1.2` 的版本约束后缀：去掉最后一段以 `@^` 开头的部分。
                let trimmed = match text.rfind("@^") {
                    Some(at) if !text[at + 2..].contains('@') => &text[..at],
                    _ => text.as_str(),
                };
                Some(trimmed.to_owned())
            }
            Json::Object(_) => {
                let name = item.get("name").and_then(Json::as_str)?.trim().to_owned();
                if name.is_empty() {
                    return None;
                }
                let market = item
                    .get("marketplace")
                    .and_then(Json::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                Some(if market.is_empty() {
                    name
                } else {
                    format!("{name}@{market}")
                })
            }
            _ => None,
        })
        .collect()
}

/// TS `qualifyDependency`：未带市场的依赖属于条目所在市场。
pub(super) fn dependencies(entry: &Json, marketplace: &str) -> Vec<String> {
    normalized_dependencies(entry)
        .into_iter()
        .map(|dep| {
            if dep.contains('@') {
                dep
            } else {
                format!("{dep}@{marketplace}")
            }
        })
        .collect()
}

pub(super) fn split_id(id: &str) -> Result<(&str, &str)> {
    match id.rfind('@') {
        Some(at) if at > 0 && at < id.len() - 1 => Ok((&id[..at], &id[at + 1..])),
        _ => bail!("Plugin id must use <name>@<marketplace>: {id}"),
    }
}

/// TS `resolveDependencyClosure`：深度优先、后序（依赖先于依赖者）；跨市场需根市场白名单。
pub(super) fn closure(storage: &Path, marketplace: &str, name: &str) -> Result<Vec<String>> {
    closure_in(storage, marketplace, name, None)
}

/// `local`：根市场的 manifest 原文（TS `resolveDependencyClosureFromManifest`，市场尚未落盘时用）；
/// 其它市场仍从存储读取。
pub(super) fn closure_in(
    storage: &Path,
    marketplace: &str,
    name: &str,
    local: Option<&Json>,
) -> Result<Vec<String>> {
    let allow: Vec<String> = local
        .cloned()
        .or_else(|| ordered_manifest(storage, marketplace))
        .and_then(|m| m.get("allowCrossMarketplaceDependenciesOn").cloned())
        .and_then(|v| {
            v.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        })
        .unwrap_or_default();
    let root = format!("{name}@{marketplace}");
    let mut out = vec![];
    let mut visiting: Vec<String> = vec![];
    #[allow(clippy::too_many_arguments)]
    fn walk(
        storage: &Path,
        local: Option<&Json>,
        root_market: &str,
        allow: &[String],
        id: &str,
        required_by: &str,
        visiting: &mut Vec<String>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        let (name, market) = split_id(id)?;
        if market != root_market && !allow.iter().any(|a| a == market) {
            bail!("Cross-marketplace dependency is blocked: {id} required by {required_by}");
        }
        if visiting.iter().any(|v| v == id) {
            bail!("Plugin dependency cycle: {} -> {id}", visiting.join(" -> "));
        }
        if out.iter().any(|v| v == id) {
            return Ok(());
        }
        let entry = match local.filter(|_| market == root_market) {
            Some(manifest) => entry_in(manifest, name),
            None => {
                if market::manifest(storage, market).is_none() {
                    bail!("Marketplace not found for dependency: {market}");
                }
                ordered_entry(storage, market, name)
            }
        }
        .ok_or_else(|| anyhow!("Dependency not found: {id} required by {required_by}"))?;
        visiting.push(id.to_owned());
        for dependency in dependencies(&entry, market) {
            walk(
                storage,
                local,
                root_market,
                allow,
                &dependency,
                id,
                visiting,
                out,
            )?;
        }
        visiting.pop();
        out.push(id.to_owned());
        Ok(())
    }
    walk(
        storage,
        local,
        marketplace,
        &allow,
        &root,
        &root,
        &mut visiting,
        &mut out,
    )?;
    Ok(out)
}

/// TS `findPluginManifestPath`。
pub(super) fn manifest_path(root: &Path) -> Option<PathBuf> {
    [".escode-plugin", ".claude-plugin", ".codex-plugin"]
        .iter()
        .map(|dir| root.join(dir).join("plugin.json"))
        .find(|path| path.is_file())
}

/// TS `resolveInside`：相对路径解析后必须仍在 base 内。
pub(super) fn resolve_inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let joined = super::lexical_path::normalize(&base.join(relative));
    joined
        .starts_with(super::lexical_path::normalize(base))
        .then_some(joined)
}

/// TS `PLUGIN_NAME_PATTERN`。
pub(super) fn valid_plugin_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
}

/// TS `getPluginCacheDir`。
pub(super) fn cache_dir(storage: &Path, marketplace: &str, name: &str, version: &str) -> PathBuf {
    storage
        .join("cache")
        .join(sanitize(marketplace))
        .join(sanitize(name))
        .join(sanitize(version))
}

/// TS `resolveInstalledPluginVersion`：源根 plugin.json 的非空 version > 条目 version > 0.0.0。
pub(super) fn installed_version(root: &Path, entry: &Json) -> String {
    let from_manifest = manifest_path(root)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value["version"].as_str().map(str::to_owned))
        .filter(|v| !v.trim().is_empty());
    from_manifest.unwrap_or_else(|| {
        entry
            .get("version")
            .and_then(Json::as_str)
            .unwrap_or("0.0.0")
            .to_owned()
    })
}
