//! 用户级内联插件候选、manifest 与配置读写（TS `pluginSyncService.ts:217-269,404-436,907-1034`）。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::skills::Compare;

pub use super::config::{
    add_plugin_dir, config_path, marketplace_root, plugin_root, read_user_config, resolve_path,
};

const INLINE_MARKETPLACE: &str = "inline";
const MANIFEST_PATHS: [[&str; 2]; 4] = [
    [".escode-plugin", "plugin.json"],
    [".zcode-plugin", "plugin.json"],
    [".claude-plugin", "plugin.json"],
    [".codex-plugin", "plugin.json"],
];

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub plugin_id: String,
    pub directory_name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub enabled: bool,
    pub enabled_override: Option<bool>,
    pub component_types: Vec<&'static str>,
}

impl Candidate {
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".to_owned(), Value::String(self.id.clone()));
        object.insert("name".to_owned(), Value::String(self.name.clone()));
        object.insert("pluginId".to_owned(), Value::String(self.plugin_id.clone()));
        object.insert(
            "directoryName".to_owned(),
            Value::String(self.directory_name.clone()),
        );
        if let Some(description) = &self.description {
            object.insert("description".to_owned(), Value::String(description.clone()));
        }
        if let Some(version) = &self.version {
            object.insert("version".to_owned(), Value::String(version.clone()));
        }
        object.insert(
            "path".to_owned(),
            Value::String(self.path.to_string_lossy().into_owned()),
        );
        object.insert("sizeBytes".to_owned(), Value::from(self.size_bytes));
        object.insert("enabled".to_owned(), Value::Bool(self.enabled));
        if let Some(enabled_override) = self.enabled_override {
            object.insert("enabledOverride".to_owned(), Value::Bool(enabled_override));
        }
        object.insert("componentTypes".to_owned(), json!(self.component_types));
        Value::Object(object)
    }
}

pub struct Manifest {
    pub name: String,
    pub plugin_id: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub component_types: Vec<&'static str>,
}

pub fn collect(home: &Path, compare: &Compare) -> Result<Vec<Candidate>, String> {
    let config = read_user_config(home)?;
    let plugins = config.get("plugins").and_then(Value::as_object);
    let dirs = plugins
        .and_then(|plugins| plugins.get("dirs"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.trim().is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let enabled = enabled_overrides(plugins)?;
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    let mut used_names = HashSet::new();
    for raw_dir in dirs {
        let resolved = resolve_path(home, &raw_dir);
        let canonical = std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
        if !seen.insert(canonical.to_string_lossy().into_owned()) {
            continue;
        }
        let Some(manifest) = read_manifest(&canonical)? else {
            continue;
        };
        let override_value = enabled.get(&manifest.plugin_id.to_lowercase()).copied();
        let base_name = canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| manifest.name.clone());
        let directory_name =
            unique_directory_name(&base_name, &manifest.name, &canonical, &mut used_names);
        candidates.push(Candidate {
            id: short_hash(&format!(
                "{}:{}",
                manifest.plugin_id,
                canonical.to_string_lossy()
            )),
            name: manifest.name.clone(),
            plugin_id: manifest.plugin_id.clone(),
            directory_name,
            description: manifest.description.clone(),
            version: manifest.version.clone(),
            path: canonical.clone(),
            size_bytes: recursive_size(&canonical).unwrap_or(0),
            enabled: override_value.unwrap_or(true),
            enabled_override: override_value,
            component_types: manifest.component_types,
        });
    }
    candidates.sort_by(|left, right| compare(&left.name, &right.name));
    Ok(candidates)
}

pub fn configured_paths(home: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let config = read_user_config(home)?;
    let dirs = config
        .get("plugins")
        .and_then(Value::as_object)
        .and_then(|plugins| plugins.get("dirs"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut result = BTreeMap::new();
    for raw in dirs.iter().filter_map(Value::as_str) {
        if raw.trim().is_empty() {
            continue;
        }
        let resolved = resolve_path(home, raw);
        let canonical = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        let Some(manifest) = read_manifest(&canonical)? else {
            continue;
        };
        result
            .entry(manifest.plugin_id.to_lowercase())
            .or_insert(canonical);
    }
    Ok(result)
}

pub fn read_manifest(root: &Path) -> Result<Option<Manifest>, String> {
    for segments in MANIFEST_PATHS {
        let path = root.join(segments[0]).join(segments[1]);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Ok(None);
        };
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let Some(name) = name else {
            return Ok(None);
        };
        let component_types = component_types(root, object);
        return Ok(Some(Manifest {
            name: name.to_owned(),
            plugin_id: format!("{name}@{INLINE_MARKETPLACE}"),
            description: trimmed(object.get("description")),
            version: trimmed(object.get("version")),
            component_types,
        }));
    }
    Ok(None)
}

fn component_types(root: &Path, manifest: &Map<String, Value>) -> Vec<&'static str> {
    let mut types = Vec::new();
    if manifest.contains_key("skills") || exists(&root.join("skills")) {
        types.push("skills");
    }
    if manifest.contains_key("commands") || exists(&root.join("commands")) {
        types.push("commands");
    }
    if manifest.contains_key("hooks") || exists(&root.join("hooks").join("hooks.json")) {
        types.push("hooks");
    }
    if manifest.contains_key("mcpServers") || exists(&root.join(".mcp.json")) {
        types.push("mcp");
    }
    types
}

fn trimmed(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn enabled_overrides(
    plugins: Option<&Map<String, Value>>,
) -> Result<BTreeMap<String, bool>, String> {
    let mut result = BTreeMap::new();
    if let Some(values) = plugins
        .and_then(|plugins| plugins.get("enabledPlugins"))
        .and_then(Value::as_object)
    {
        for (plugin_id, value) in values {
            if let Some(enabled) = value.as_bool() {
                let normalized = normalize_plugin_id(plugin_id)?;
                result.insert(normalized.to_lowercase(), enabled);
            }
        }
    }
    Ok(result)
}

pub fn normalize_plugin_id(value: &str) -> Result<String, String> {
    let normalized = value.trim();
    if normalized.is_empty() || normalized.contains('/') || normalized.contains('\\') {
        return Err(format!("invalid plugin id: {value}"));
    }
    Ok(normalized.to_owned())
}

pub fn unique_directory_name(
    directory_basename: &str,
    plugin_name: &str,
    plugin_root: &Path,
    used: &mut HashSet<String>,
) -> String {
    let base = safe_directory_name(if directory_basename.is_empty() {
        plugin_name
    } else {
        directory_basename
    });
    if used.insert(base.clone()) {
        return base;
    }
    let deduped = safe_directory_name(&format!(
        "{base}-{}",
        &short_hash(&plugin_root.to_string_lossy())[..8]
    ));
    used.insert(deduped.clone());
    deduped
}

pub fn safe_directory_name(value: &str) -> String {
    let sanitized = {
        let text = value.trim();
        let replaced: String = text
            .chars()
            .map(|character| match character {
                '\\' | '/' | ':' => '-',
                other => other,
            })
            .collect();
        if replaced.is_empty() {
            "plugin".to_owned()
        } else {
            replaced
        }
    };
    super::paths::normalize_relative(&sanitized, "unsafe plugin sync path")
        .unwrap_or_else(|_| format!("plugin-{}", &short_hash(value)[..8]))
}

pub fn recursive_size(path: &Path) -> Result<u64, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        total = total.saturating_add(recursive_size(&entry.path())?);
    }
    Ok(total)
}

pub fn content_fingerprint(path: &Path, compare: &Compare) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    if metadata.is_file() {
        hash.update(b"file\0");
        hash.update(std::fs::read(path).map_err(|error| error.to_string())?);
        return Ok(hex(hash.finalize()));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "unsupported plugin archive source: {}",
            path.display()
        ));
    }
    hash.update(b"dir\0");
    let mut children: Vec<std::fs::DirEntry> = std::fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    children.sort_by(|left, right| {
        compare(
            &left.file_name().to_string_lossy(),
            &right.file_name().to_string_lossy(),
        )
    });
    for child in children {
        hash.update(child.file_name().to_string_lossy().as_bytes());
        hash.update(b"\0");
        hash.update(content_fingerprint(&child.path(), compare)?.as_bytes());
        hash.update(b"\0");
    }
    Ok(hex(hash.finalize()))
}

pub fn short_hash(value: &str) -> String {
    hex(Sha256::digest(value.as_bytes()))
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}
