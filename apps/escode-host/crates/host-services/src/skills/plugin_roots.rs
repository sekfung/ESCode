//! 插件贡献的技能根（TS `resolvePluginSkillRootDescriptors`）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::discover::{Compare, Root, exists};
use super::policy;
use super::walk::sorted_entries;
use crate::git::paths::js_trim;

const OFFICIAL_MARKETPLACE: &str = "zcode-plugins-official";
const INLINE_MARKETPLACE: &str = "inline";
const MANIFESTS: [&str; 4] = [
    ".escode-plugin/plugin.json",
    ".zcode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
];

pub struct PluginConfig {
    dirs: Vec<String>,
    enabled: bool,
    enabled_plugins: BTreeMap<String, bool>,
    storage_dir: String,
    suppressed: Vec<String>,
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn plugin_config(config: &Value) -> PluginConfig {
    let plugins = config.get("plugins").filter(|v| v.is_object());
    let get = |key: &str| plugins.and_then(|p| p.get(key));
    let storage_dir = config
        .get("storage")
        .and_then(|s| s.get("dir"))
        .and_then(Value::as_str)
        .filter(|d| !js_trim(d).is_empty())
        .unwrap_or("~/.escode")
        .to_owned();
    PluginConfig {
        dirs: string_list(get("dirs")),
        enabled: get("enabled").and_then(Value::as_bool).unwrap_or(true),
        enabled_plugins: get("enabledPlugins")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                    .collect()
            })
            .unwrap_or_default(),
        storage_dir,
        suppressed: string_list(get("suppressedBuiltins")),
    }
}

fn resolve_config_path(path: &str, home: &Path) -> PathBuf {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::path::absolute(&expanded).unwrap_or(expanded)
    }
}

/// `path.resolve` 的词法规整（折叠 `.`/`..`）。
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_inside(root: &Path, raw: &str) -> Option<PathBuf> {
    if Path::new(raw).is_absolute() {
        return None;
    }
    let resolved = lexical(&root.join(raw));
    let relative = resolved.strip_prefix(lexical(root)).ok()?;
    let text = relative.to_string_lossy();
    (text.is_empty() || (!text.starts_with("..") && !text.contains("../"))).then_some(resolved)
}

struct Candidate {
    default_enabled: bool,
    marketplace: String,
    root: PathBuf,
}

fn official_cache_roots(storage_root: &Path, compare: &Compare) -> Vec<PathBuf> {
    let cache = storage_root.join("cache").join(OFFICIAL_MARKETPLACE);
    let Ok(plugins) = sorted_entries(&cache) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for plugin in plugins {
        if !plugin.file_type().is_ok_and(|k| k.is_dir()) {
            continue;
        }
        let dir = cache.join(plugin.file_name());
        let Ok(versions) = sorted_entries(&dir) else {
            continue;
        };
        roots.extend(
            versions
                .into_iter()
                .filter(|v| v.file_type().is_ok_and(|k| k.is_dir()))
                .map(|v| dir.join(v.file_name())),
        );
    }
    roots.sort_by(|a, b| compare(&a.to_string_lossy(), &b.to_string_lossy()));
    roots
}

fn installed_roots(storage_root: &Path) -> Vec<Candidate> {
    let Ok(text) = std::fs::read_to_string(storage_root.join("installed_plugins.json")) else {
        return Vec::new();
    };
    let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(items) = parsed.get("plugins").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .map(js_trim)
                    .unwrap_or_default()
                    .to_owned()
            };
            let (id, marketplace, install) = (text("id"), text("marketplace"), text("installPath"));
            (!id.is_empty()
                && !marketplace.is_empty()
                && !install.is_empty()
                && Path::new(&install).is_absolute())
            .then(|| Candidate {
                default_enabled: false,
                marketplace,
                root: PathBuf::from(install),
            })
        })
        .collect()
}

fn read_manifest(root: &Path) -> Option<(String, Option<Value>)> {
    let path = MANIFESTS.iter().map(|m| root.join(m)).find(|p| exists(p))?;
    let parsed: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let object = parsed.as_object()?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(js_trim)
        .unwrap_or_default()
        .to_owned();
    valid_plugin_name(&name).then(|| (name, object.get("skills").cloned()))
}

/// `/^[a-z0-9][a-z0-9._-]{0,127}$/`。
fn valid_plugin_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && name.len() <= 128
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn plugin_skill_roots(skills: Option<&Value>, root: &Path) -> Vec<PathBuf> {
    let paths: Vec<String> = match skills {
        Some(Value::String(text)) => vec![text.clone()],
        other => string_list(other),
    };
    let mut roots: Vec<PathBuf> = paths
        .iter()
        .filter_map(|raw| resolve_inside(root, raw))
        .collect();
    if roots.is_empty() && skills.is_none() {
        let default_root = root.join("skills");
        if exists(&default_root) {
            roots.push(default_root);
        }
    }
    roots
}

pub(super) fn plugin_roots(config: &Value, home: &Path, compare: &Compare) -> Vec<Root> {
    let config = plugin_config(config);
    if !config.enabled {
        return Vec::new();
    }
    let storage = resolve_config_path(&config.storage_dir, home);
    let cli_root = if storage.file_name().is_some_and(|n| n == "cli") {
        storage
    } else {
        storage.join("cli")
    };
    let plugin_storage = cli_root.join("plugins");
    let mut candidates: Vec<Candidate> = config
        .dirs
        .iter()
        .map(|dir| Candidate {
            default_enabled: true,
            marketplace: INLINE_MARKETPLACE.to_owned(),
            root: resolve_config_path(dir, home),
        })
        .collect();
    candidates.extend(
        official_cache_roots(&plugin_storage, compare)
            .into_iter()
            .map(|root| Candidate {
                default_enabled: false,
                marketplace: OFFICIAL_MARKETPLACE.to_owned(),
                root,
            }),
    );
    candidates.extend(installed_roots(&plugin_storage));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let Some((name, skills)) = read_manifest(&candidate.root) else {
            continue;
        };
        let id = format!("{name}@{}", candidate.marketplace);
        if candidate.marketplace == OFFICIAL_MARKETPLACE && config.suppressed.contains(&id) {
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let default_enabled =
            candidate.default_enabled || policy::DEFAULT_ENABLED_PLUGIN_IDS.contains(&id.as_str());
        if !config
            .enabled_plugins
            .get(&id)
            .copied()
            .unwrap_or(default_enabled)
        {
            continue;
        }
        for path in plugin_skill_roots(skills.as_ref(), &candidate.root) {
            out.push(Root {
                scope: "plugin",
                path,
                plugin_name: Some(name.clone()),
                plugin_id: Some(id.clone()),
            });
        }
    }
    out
}
