//! marketplace source 镜像归档（TS `pluginSyncService.ts:271-402,566-905`）。
//!
//! 只镜像本地源：选中插件及其同 marketplace 依赖复制到归档的 `plugins/<name>`，
//! manifest 的 `metadata.pluginRoot` 改写为 `plugins`；github/git/url/npm/pip 等远端源保持原样。

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ordered_json::{self, OrderedJson};

use super::archive::{ArchiveEntry, create_archive};
use super::candidates::{self, safe_directory_name};
use super::marketplace_manifest::{
    Loaded, Manifest, PluginEntry, closure, load_source, require_string,
};
use super::paths;

const MIRRORED_ROOT: &str = "plugins";

#[derive(Clone)]
struct MirrorEntry {
    entry: PluginEntry,
    mirrored_source_path: Option<PathBuf>,
    mirrored_source_relative_path: Option<String>,
}

pub struct ExportResult {
    pub archive: Vec<u8>,
    pub bytes: u64,
    pub marketplace_id: String,
    pub plugin_names: Vec<String>,
}

pub fn export(
    home: &Path,
    params: &Value,
    max_bytes: u64,
    compare: &crate::skills::Compare,
) -> Result<ExportResult, String> {
    let marketplace_id = require_string(params.get("marketplaceId"), "marketplace id")?;
    let names: Vec<String> = params
        .get("pluginNames")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| require_string(Some(&Value::String(item.to_owned())), "plugin name"))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let mut unique = Vec::new();
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    if unique.is_empty() {
        return Err("missing marketplace plugin names".to_owned());
    }
    let source = params
        .get("source")
        .and_then(Value::as_object)
        .ok_or("marketplace source is invalid")?;
    let loaded = load_source(source, home)?;
    if loaded.manifest.name != marketplace_id {
        return Err(format!(
            "marketplace id mismatch: {} !== {marketplace_id}",
            loaded.manifest.name
        ));
    }
    let closure = closure(&loaded.manifest, &marketplace_id, &unique)?;
    let mut mirror = Vec::with_capacity(closure.len());
    for entry in &closure {
        mirror.push(mirror_entry(&loaded, entry)?);
    }
    let manifest = mirrored_manifest(&loaded.manifest, &mirror);
    let manifest_content = ordered_json::to_pretty(&OrderedJson::Object(manifest))
        .map_err(|error| error.to_string())?;
    let mut selected_bytes = manifest_content.len() as u64;
    for entry in &mirror {
        if let Some(path) = &entry.mirrored_source_path {
            selected_bytes = selected_bytes.saturating_add(candidates::recursive_size(path)?);
        }
    }
    if selected_bytes > max_bytes {
        return Err(format!(
            "plugin marketplace source archive exceeds limit: {selected_bytes}/{max_bytes}"
        ));
    }
    let fingerprints: Result<Vec<Option<String>>, String> = mirror
        .iter()
        .map(|entry| match &entry.mirrored_source_path {
            Some(path) => candidates::content_fingerprint(path, compare).map(Some),
            None => Ok(None),
        })
        .collect();
    let directory_name = source_directory_name(&marketplace_id, &manifest_content, &fingerprints?);
    let mut entries = vec![ArchiveEntry::Memory {
        archive_path: format!("{directory_name}/marketplace.json"),
        content: manifest_content.into_bytes(),
        mode: 0o644,
        mtime_ms: None,
    }];
    for entry in &mirror {
        if let (Some(source_path), Some(relative)) = (
            &entry.mirrored_source_path,
            &entry.mirrored_source_relative_path,
        ) {
            entries.push(ArchiveEntry::Source {
                source_path: source_path.clone(),
                archive_path: format!("{directory_name}/{relative}"),
            });
        }
    }
    let metadata = ordered_json::to_pretty(&metadata(&marketplace_id, &directory_name))
        .map_err(|error| error.to_string())?;
    let archive = create_archive(&entries, &metadata, compare)?;
    if archive.len() as u64 > max_bytes {
        return Err(format!(
            "plugin marketplace source archive exceeds limit: {}/{max_bytes}",
            archive.len()
        ));
    }
    Ok(ExportResult {
        bytes: archive.len() as u64,
        archive,
        marketplace_id,
        plugin_names: closure.iter().map(|entry| entry.name.clone()).collect(),
    })
}

fn mirror_entry(loaded: &Loaded, entry: &PluginEntry) -> Result<MirrorEntry, String> {
    if is_remote_source(entry.source.as_ref()) {
        return Ok(MirrorEntry {
            entry: entry.clone(),
            mirrored_source_path: None,
            mirrored_source_relative_path: None,
        });
    }
    let source_path = resolve_local_source(loaded, entry)?;
    let relative = format!("{MIRRORED_ROOT}/{}", safe_directory_name(&entry.name));
    Ok(MirrorEntry {
        entry: entry.clone(),
        mirrored_source_path: Some(source_path),
        mirrored_source_relative_path: Some(relative),
    })
}

fn is_remote_source(source: Option<&OrderedJson>) -> bool {
    let Some(OrderedJson::Object(object)) = source else {
        return false;
    };
    let kind = object.get("source").and_then(|value| match value {
        OrderedJson::String(text) => Some(text.as_str()),
        _ => None,
    });
    matches!(
        kind,
        Some("github" | "git" | "url" | "git-subdir" | "npm" | "pip")
    )
}

fn resolve_local_source(loaded: &Loaded, entry: &PluginEntry) -> Result<PathBuf, String> {
    match &entry.source {
        Some(OrderedJson::String(source)) => {
            resolve_source_path(loaded, source, &format!("{} source", entry.name), false)
        }
        Some(OrderedJson::Object(object)) => {
            let kind = object.get("source").and_then(|value| match value {
                OrderedJson::String(text) => Some(text.as_str()),
                _ => None,
            });
            if kind == Some("directory") {
                let path = object
                    .get("path")
                    .and_then(|value| match value {
                        OrderedJson::String(text) => Some(text.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| format!("missing {} directory source", entry.name))?;
                resolve_source_path(
                    loaded,
                    &path,
                    &format!("{} directory source", entry.name),
                    loaded.source_root.is_none(),
                )
            } else {
                fallback_source(loaded, entry)
            }
        }
        _ => fallback_source(loaded, entry),
    }
}

fn fallback_source(loaded: &Loaded, entry: &PluginEntry) -> Result<PathBuf, String> {
    let Some(root) = &loaded.source_root else {
        return Err(format!(
            "cannot mirror local marketplace plugin source without source root: {}",
            entry.name
        ));
    };
    let base = plugin_base_dir(loaded).unwrap_or_else(|| root.clone());
    let candidate =
        paths::resolve_within(&base, &entry.name, "unsafe marketplace plugin source path")?;
    if candidate.is_dir() {
        assert_source_root(&candidate)?;
        Ok(candidate)
    } else {
        Err(format!(
            "local marketplace plugin source does not exist: {}",
            candidate.display()
        ))
    }
}

fn resolve_source_path(
    loaded: &Loaded,
    source: &str,
    label: &str,
    allow_absolute: bool,
) -> Result<PathBuf, String> {
    let requested = source.trim();
    if requested.is_empty() {
        return Err(format!("missing {label}"));
    }
    let relative = requested.strip_prefix("./").unwrap_or(requested);
    if loaded.source_root.is_none() {
        if !allow_absolute {
            return Err(format!(
                "cannot mirror local marketplace plugin source without source root: {source}"
            ));
        }
        let path = candidates::resolve_path(Path::new("/"), requested);
        if path.is_dir() {
            assert_source_root(&path)?;
            return Ok(path);
        }
        return Err(format!(
            "local marketplace plugin source does not exist: {source}"
        ));
    }
    let base = plugin_base_dir(loaded)
        .ok_or_else(|| "cannot resolve marketplace plugin source root".to_owned())?;
    let candidate =
        paths::resolve_within(&base, relative, "unsafe marketplace plugin source path")?;
    if candidate.is_dir() {
        assert_source_root(&candidate)?;
        Ok(candidate)
    } else {
        Err(format!(
            "local marketplace plugin source does not exist: {source}"
        ))
    }
}

fn plugin_base_dir(loaded: &Loaded) -> Option<PathBuf> {
    let root = loaded.source_root.as_ref()?;
    let Some(plugin_root) = &loaded.manifest.plugin_root else {
        return Some(root.clone());
    };
    let candidate =
        paths::resolve_within(root, plugin_root, "unsafe marketplace pluginRoot").ok()?;
    Some(if candidate.exists() {
        candidate
    } else {
        root.clone()
    })
}

fn assert_source_root(root: &Path) -> Result<(), String> {
    if candidates::read_manifest(root)?.is_some() {
        Ok(())
    } else {
        Err(format!(
            "local marketplace plugin source is missing plugin manifest: {}",
            root.display()
        ))
    }
}

fn mirrored_manifest(
    manifest: &Manifest,
    entries: &[MirrorEntry],
) -> IndexMap<String, OrderedJson> {
    let has_mirrored = entries
        .iter()
        .any(|entry| entry.mirrored_source_relative_path.is_some());
    let mut raw = manifest.raw.clone();
    raw.shift_remove("pluginRoot");
    raw.insert(
        "name".to_owned(),
        OrderedJson::String(manifest.name.clone()),
    );
    if let Some(description) = &manifest.description {
        raw.insert(
            "description".to_owned(),
            OrderedJson::String(description.clone()),
        );
    }
    if has_mirrored {
        let mut metadata = match raw.get("metadata") {
            Some(OrderedJson::Object(map)) => map.clone(),
            _ => IndexMap::new(),
        };
        metadata.insert(
            "pluginRoot".to_owned(),
            OrderedJson::String(MIRRORED_ROOT.to_owned()),
        );
        raw.insert("metadata".to_owned(), OrderedJson::Object(metadata));
    }
    let plugins = entries
        .iter()
        .map(|entry| {
            let mut raw = entry.entry.raw.clone();
            raw.insert(
                "name".to_owned(),
                OrderedJson::String(entry.entry.name.clone()),
            );
            if let Some(relative) = &entry.mirrored_source_relative_path {
                let name = relative.split('/').nth(1).unwrap_or(relative);
                raw.insert(
                    "source".to_owned(),
                    OrderedJson::String(format!("./{name}")),
                );
            }
            OrderedJson::Object(raw)
        })
        .collect();
    raw.insert("plugins".to_owned(), OrderedJson::Array(plugins));
    raw
}

fn source_directory_name(
    marketplace_id: &str,
    manifest_content: &str,
    fingerprints: &[Option<String>],
) -> String {
    let payload = serde_json::json!({
        "manifestContent": manifest_content,
        "sourceFingerprints": fingerprints,
    });
    let hash = Sha256::digest(payload.to_string().as_bytes());
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}", safe_directory_name(marketplace_id), &hex[..12])
}

fn metadata(marketplace_id: &str, directory_name: &str) -> OrderedJson {
    let mut source = IndexMap::new();
    source.insert(
        "marketplaceId".to_owned(),
        OrderedJson::String(marketplace_id.to_owned()),
    );
    source.insert(
        "directoryName".to_owned(),
        OrderedJson::String(directory_name.to_owned()),
    );
    let mut root = IndexMap::new();
    root.insert(
        "marketplaceSources".to_owned(),
        OrderedJson::Array(vec![OrderedJson::Object(source)]),
    );
    OrderedJson::Object(root)
}
