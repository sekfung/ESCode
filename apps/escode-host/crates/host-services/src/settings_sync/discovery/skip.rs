//! 判重（TS `get*CandidateSkipReason` / `collectExisting*NameKeys`）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::super::candidates::{collect_command_paths, exists_dir, name_key, path_exists};
use super::super::sources::SourceScope;

#[derive(Default)]
pub struct SkipCaches {
    skill_names: BTreeMap<PathBuf, BTreeSet<String>>,
    command_names: BTreeMap<PathBuf, BTreeSet<String>>,
    plugin_ids: BTreeMap<PathBuf, BTreeSet<String>>,
    mcp_names: BTreeMap<SourceScope, BTreeSet<String>>,
}

pub fn skill_skip(
    target_root: &Path,
    target_path: &Path,
    name_key: &str,
    caches: &mut SkipCaches,
) -> Option<&'static str> {
    if exists_dir(target_path) {
        return Some("targetExists");
    }
    let names = caches
        .skill_names
        .entry(target_root.to_path_buf())
        .or_insert_with(|| existing_skill_names(target_root));
    names.contains(name_key).then_some("sameNameExists")
}

pub fn command_skip(
    target_root: &Path,
    target_path: &Path,
    name_key: &str,
    caches: &mut SkipCaches,
) -> Option<&'static str> {
    if path_exists(target_path) {
        return Some("targetExists");
    }
    let names = caches
        .command_names
        .entry(target_root.to_path_buf())
        .or_insert_with(|| existing_command_names(target_root));
    names.contains(name_key).then_some("sameNameExists")
}

pub fn plugin_skip(
    target_path: &Path,
    id: &str,
    config_path: Option<&Path>,
    caches: &mut SkipCaches,
) -> Option<&'static str> {
    if path_exists(target_path) {
        return Some("targetExists");
    }
    let Some(config_path) = config_path else {
        return Some("targetExists");
    };
    let ids = caches
        .plugin_ids
        .entry(config_path.to_path_buf())
        .or_insert_with(|| existing_plugin_ids(config_path));
    ids.contains(&id.to_lowercase()).then_some("sameNameExists")
}

pub fn mcp_skip(
    target_scope: SourceScope,
    home: &Path,
    workspace: Option<&Path>,
    name_key: &str,
    caches: &mut SkipCaches,
) -> Option<&'static str> {
    let names = caches
        .mcp_names
        .entry(target_scope)
        .or_insert_with(|| existing_mcp_names(home, workspace, target_scope));
    names.contains(name_key).then_some("sameNameExists")
}

impl SkipCaches {
    pub fn remember_skill(&mut self, target_root: &Path, name_key: &str) {
        if let Some(names) = self.skill_names.get_mut(target_root) {
            names.insert(name_key.to_owned());
        }
    }

    pub fn remember_command(&mut self, target_root: &Path, name_key: &str) {
        if let Some(names) = self.command_names.get_mut(target_root) {
            names.insert(name_key.to_owned());
        }
    }

    pub fn remember_plugin(&mut self, config_path: &Path, id: &str) {
        if let Some(ids) = self.plugin_ids.get_mut(config_path) {
            ids.insert(id.to_lowercase());
        }
    }

    pub fn remember_mcp(&mut self, target_scope: SourceScope, name_key: &str) {
        if let Some(names) = self.mcp_names.get_mut(&target_scope) {
            names.insert(name_key.to_owned());
        }
    }
}

fn existing_skill_names(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if !exists_dir(root) {
        return names;
    }
    for path in crate::skills::walk_markdown_paths(root) {
        names.insert(name_key(
            &super::super::metadata::read_skill_metadata(&path).name,
        ));
    }
    names
}

fn existing_command_names(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if !exists_dir(root) {
        return names;
    }
    for path in collect_command_paths(root) {
        if let Ok(relative) = path.strip_prefix(root) {
            let relative = super::super::metadata::relative_string(relative);
            names.insert(command_name_key(&relative));
        }
    }
    names
}

pub fn command_name_key(relative: &str) -> String {
    let without_ext =
        if relative.len() >= 3 && relative[relative.len() - 3..].eq_ignore_ascii_case(".md") {
            &relative[..relative.len() - 3]
        } else {
            relative
        };
    format!("/{without_ext}").trim().to_lowercase()
}

fn existing_plugin_ids(config_path: &Path) -> BTreeSet<String> {
    let parsed = read_json_object(config_path);
    let mut ids = BTreeSet::new();
    let Some(dirs) = parsed
        .get("plugins")
        .and_then(Value::as_object)
        .and_then(|plugins| plugins.get("dirs"))
        .and_then(Value::as_array)
    else {
        return ids;
    };
    for dir in dirs.iter().filter_map(Value::as_str) {
        if let Some(metadata) = super::super::metadata::read_plugin_metadata(&resolve_lexical(dir))
        {
            ids.insert(metadata.id.to_lowercase());
        }
    }
    ids
}

fn existing_mcp_names(
    home: &Path,
    workspace: Option<&Path>,
    scope: SourceScope,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(path) = super::super::candidates::config_path(home, workspace, scope) else {
        return names;
    };
    let parsed = read_json_object(&path);
    if let Some(servers) = parsed
        .get("mcp")
        .and_then(Value::as_object)
        .and_then(|mcp| mcp.get("servers"))
        .and_then(Value::as_object)
    {
        for name in servers.keys() {
            names.insert(name_key(name));
        }
    }
    names
}

/// TS `readJsonFileOrEmpty`：缺失/损坏/非对象都当空对象。
pub fn read_json_object(path: &Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    }
}

/// Node `path.resolve` 的词典序版本（不访问文件系统）。
pub fn resolve_lexical(path: &str) -> PathBuf {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut parts = PathBuf::new();
    for component in absolute.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            other => parts.push(other.as_os_str()),
        }
    }
    parts
}
