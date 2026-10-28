//! 技能发现（TS `skillsService.ts` 的 `discoverSkills` 及根解析、插件根解析）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::frontmatter;
use super::plugin_roots::plugin_roots;
use super::policy;
use super::walk::walk;
use crate::git::paths::js_trim;

pub type Compare = std::sync::Arc<dyn Fn(&str, &str) -> std::cmp::Ordering + Send + Sync>;

#[derive(Clone)]
pub struct Root {
    pub scope: &'static str,
    pub path: PathBuf,
    pub plugin_name: Option<String>,
    pub plugin_id: Option<String>,
}

pub(super) fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

fn norm_scan_root(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

pub fn user_escode_root(home: &Path) -> PathBuf {
    home.join(".escode").join("skills")
}

pub fn user_agents_root(home: &Path) -> PathBuf {
    home.join(".agents").join("skills")
}

pub fn workspace_escode_root(workspace: &Path) -> PathBuf {
    workspace.join(".escode").join("skills")
}

/// TS `findWorktreeRoot` + `resolveAncestorWorkspaceRoots`：沿父目录走到含 `.git` 的目录。
pub fn ancestor_workspace_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut worktree = None;
    let mut current = workspace.to_path_buf();
    loop {
        if exists(&current.join(".git")) {
            worktree = Some(current);
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    let mut dirs = Vec::new();
    match worktree {
        None => dirs.push(workspace.to_path_buf()),
        Some(root) => {
            let mut current = workspace.to_path_buf();
            loop {
                dirs.push(current.clone());
                if current == root {
                    break;
                }
                match current.parent() {
                    Some(parent) if parent != current => current = parent.to_path_buf(),
                    _ => break,
                }
            }
        }
    }
    dirs.into_iter()
        .flat_map(|dir| {
            [
                dir.join(".escode").join("skills"),
                dir.join(".agents").join("skills"),
            ]
        })
        .collect()
}

/// TS `filterNestedScanRoots` + realpath 去重。
fn dedupe_scan_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let existing: BTreeSet<String> = paths
        .iter()
        .filter(|p| exists(p))
        .map(|p| norm_scan_root(p))
        .collect();
    let normalized: Vec<String> = paths.iter().map(|p| norm_scan_root(p)).collect();
    let filtered: Vec<&PathBuf> = paths
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            let current = &normalized[*index];
            if !existing.contains(current) {
                return true;
            }
            !normalized.iter().enumerate().any(|(other_index, other)| {
                other_index != *index
                    && existing.contains(other)
                    && other.starts_with(&format!("{current}/"))
            })
        })
        .map(|(_, p)| p)
        .collect();
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for path in filtered {
        let mut canonical = norm_scan_root(path);
        if exists(path) {
            canonical =
                norm_scan_root(&std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()));
        }
        if seen.insert(canonical) {
            result.push(path.clone());
        }
    }
    result
}

pub fn skill_id(scope: &str, name: &str, path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("glm:{scope}:{name}:{}", &hex[..12])
}

fn name_key(path: &Path) -> String {
    let fallback = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let key = match std::fs::read(path) {
        Ok(bytes) => {
            let parsed = frontmatter::read(&String::from_utf8_lossy(&bytes));
            let raw = if parsed.has_frontmatter {
                js_trim(&parsed.name).to_owned()
            } else {
                fallback.clone()
            };
            if raw.is_empty() { fallback } else { raw }
        }
        Err(_) => fallback,
    };
    js_trim(&key).to_lowercase()
}

fn name_keys_in_root(root: &Path, compare: &Compare) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if !exists(root) {
        return keys;
    }
    let mut ignored = Vec::new();
    for path in sorted_paths(walk(root, &mut ignored), compare) {
        keys.insert(name_key(&path));
    }
    keys
}

fn sorted_paths(paths: BTreeSet<PathBuf>, compare: &Compare) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = paths.into_iter().collect();
    list.sort_by(|a, b| compare(&a.to_string_lossy(), &b.to_string_lossy()));
    list
}

// ───────── 发现 ─────────

pub struct Discovered {
    pub skills: Vec<Value>,
    pub diagnostics: Vec<Value>,
}

pub fn discover(
    workspace: &Path,
    include_user: bool,
    home: &Path,
    config: &Value,
    compare: &Compare,
) -> Discovered {
    let mut roots: Vec<Root> = Vec::new();
    let mut seen_roots = BTreeSet::new();
    for path in ancestor_workspace_roots(workspace) {
        if seen_roots.insert(path.clone()) {
            roots.push(Root {
                scope: "workspace",
                path,
                plugin_name: None,
                plugin_id: None,
            });
        }
    }
    if include_user {
        for path in [user_escode_root(home), user_agents_root(home)] {
            roots.push(Root {
                scope: "user",
                path,
                plugin_name: None,
                plugin_id: None,
            });
        }
    }
    roots.extend(plugin_roots(config, home, compare));

    let mut diagnostics: Vec<Value> = Vec::new();
    let mut skills: Vec<Value> = Vec::new();
    let mut seen_paths = BTreeSet::new();
    let user_keys = if include_user {
        name_keys_in_root(&user_escode_root(home), compare)
    } else {
        BTreeSet::new()
    };
    let scan_paths = dedupe_scan_roots(&roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>());
    // JS `Map(roots by path)`：同路径后者覆盖前者。
    let by_path: BTreeMap<&PathBuf, &Root> = roots.iter().map(|r| (&r.path, r)).collect();
    for scan in scan_paths {
        let Some(root) = by_path.get(&scan) else {
            continue;
        };
        if !exists(&root.path) {
            continue;
        }
        // 目录扫描结果按「目录 + 指纹」进程内共享；这里只做与当前 workspace 相关的过滤与物化。
        let scan = super::cache::scan_root_cached(&root.path, compare, &mut diagnostics);
        for cached in &scan.skills {
            if root.path == user_agents_root(home)
                && covered_by_escode(&cached.source_path, home, &user_keys)
            {
                continue;
            }
            if !seen_paths.insert(cached.canonical_path.clone()) || !exists(&cached.source_path) {
                continue;
            }
            let canonical_text = cached.canonical_path.to_string_lossy().into_owned();
            let mut skill = Map::new();
            skill.insert(
                "id".into(),
                json!(skill_id(root.scope, &cached.name, &canonical_text)),
            );
            skill.insert("name".into(), json!(cached.name));
            skill.insert("description".into(), json!(cached.description));
            skill.insert("body".into(), json!(cached.body));
            skill.insert("path".into(), json!(canonical_text));
            skill.insert(
                "sourcePath".into(),
                json!(cached.source_path.to_string_lossy()),
            );
            skill.insert("scope".into(), json!(root.scope));
            skill.insert("enabled".into(), json!(true));
            if let Some(plugin) = &root.plugin_name {
                skill.insert("pluginName".into(), json!(plugin));
            }
            if let Some(plugin) = &root.plugin_id {
                skill.insert("pluginId".into(), json!(plugin));
            }
            if let Some(meta) = &cached.metadata {
                skill.insert("metadata".into(), meta.clone());
            }
            skills.push(Value::Object(skill));
        }
    }
    let text = |skill: &Value, key: &str| skill[key].as_str().unwrap_or_default().to_owned();
    skills.sort_by(|l, r| {
        compare(&text(l, "name"), &text(r, "name"))
            .then_with(|| compare(&text(l, "scope"), &text(r, "scope")))
            .then_with(|| compare(&text(l, "path"), &text(r, "path")))
    });
    Discovered {
        skills,
        diagnostics,
    }
}

fn covered_by_escode(skill: &Path, home: &Path, user_keys: &BTreeSet<String>) -> bool {
    let folder = skill
        .parent()
        .and_then(Path::file_name)
        .map(std::borrow::ToOwned::to_owned)
        .unwrap_or_default();
    if exists(
        &user_escode_root(home)
            .join(folder)
            .join(policy::SKILL_FILE_NAME),
    ) {
        return true;
    }
    user_keys.contains(&name_key(skill))
}
