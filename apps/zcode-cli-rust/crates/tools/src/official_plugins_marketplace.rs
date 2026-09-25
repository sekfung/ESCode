//! 官方 marketplace：内置分片与合并目录，与 TS `official-marketplace.ts` 字节一致。
//! 见 docs/specs/rust-official-plugin-seed.md。

use std::path::Path;

use anyhow::Result;
use zcode_cli_domain::json_order::Json;

use super::official_plugins::{SeedPlugin, assets};
use super::official_plugins_cache as cache;

/// TS `writeOfficialMarketplace` + `writeBundledOfficialMarketplacePartitionSync`（含合并目录重建）。
pub(crate) fn write(storage: &Path, plugins: &[SeedPlugin]) -> Result<()> {
    let assets = assets();
    let entries = plugins
        .iter()
        .map(|plugin| {
            let definition = plugin.definition;
            let mut entry = Json::object();
            entry.set(
                "cachePath",
                Json::str(cache::cache_root(storage, definition).to_string_lossy()),
            );
            if let Some(description) = description(plugin) {
                entry.set("description", Json::str(description));
            }
            entry.set("name", Json::str(&definition.name));
            entry.set("source", Json::str("filesystem"));
            entry.set("version", Json::str(&definition.version));
            if let Some(Json::Object(listing)) = &definition.listing {
                for (key, value) in listing {
                    entry.set(key.as_str(), value.clone());
                }
            }
            entry
        })
        .collect();
    let mut manifest = Json::object();
    manifest.set("name", Json::str(&assets.marketplace));
    manifest.set("plugins", Json::Array(entries));
    manifest.set("version", Json::Number(1.into()));
    let mut partition = Json::object();
    partition.set("manifest", manifest.clone());
    partition.set("version", Json::Number(1.into()));
    let dir = storage.join("marketplaces").join(&assets.marketplace);
    write_json(&dir.join("bundled-marketplace.json"), &partition)?;
    // TS rebuildOfficialMarketplaceSync：{...bundled, ...cdn, name, plugins: [...cdn, ...bundled 去重]}。
    let cdn = std::fs::read_to_string(dir.join("cdn-marketplace.json"))
        .ok()
        .and_then(|text| Json::parse(&text))
        .filter(Json::is_object);
    let plugin_list = |m: Option<&Json>| -> Vec<Json> {
        m.and_then(|m| m.get("plugins"))
            .and_then(Json::as_array)
            .map(|items| items.iter().filter(|i| i.is_object()).cloned().collect())
            .unwrap_or_default()
    };
    let name_of = |p: &Json| {
        p.get("name")
            .and_then(Json::as_str)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    };
    let cdn_plugins = plugin_list(cdn.as_ref());
    let cdn_names: Vec<String> = cdn_plugins.iter().filter_map(name_of).collect();
    let bundled: Vec<Json> = plugin_list(Some(&manifest))
        .into_iter()
        .filter(|p| name_of(p).is_some_and(|n| !cdn_names.contains(&n)))
        .collect();
    let mut merged = manifest.clone();
    if let Some(Json::Object(entries)) = &cdn {
        for (key, value) in entries {
            merged.set(key.as_str(), value.clone());
        }
    }
    merged.set("name", Json::str(&assets.marketplace));
    merged.set(
        "plugins",
        Json::Array(cdn_plugins.into_iter().chain(bundled).collect()),
    );
    write_json(&dir.join("marketplace.json"), &merged)
}

fn description(plugin: &SeedPlugin) -> Option<String> {
    let file = plugin
        .files
        .iter()
        .find(|f| f.path == ".zcode-plugin/plugin.json")?;
    let manifest = Json::parse(&std::fs::read_to_string(&file.source).ok()?)?;
    manifest
        .get("description")
        .and_then(Json::as_str)
        .filter(|d| !d.trim().is_empty())
        .map(str::to_owned)
}

/// TS writeJsonFileSync：内容相同则不写。
fn write_json(path: &Path, value: &Json) -> Result<()> {
    let contents = format!("{}\n", value.pretty());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::read_to_string(path).ok().as_deref() == Some(contents.as_str()) {
        return Ok(());
    }
    std::fs::write(path, contents)?;
    Ok(())
}
