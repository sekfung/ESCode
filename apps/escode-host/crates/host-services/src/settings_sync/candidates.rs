//! 四类设置的候选收集（TS `collect*ImportCandidates`）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::skills::{Compare, walk_markdown_paths};

use super::agents::Agent;
use super::mcp_source::{read_source_servers, strip_timeout_fields};
use super::metadata::{
    find_plugin_manifest, read_command_metadata, read_plugin_metadata, read_skill_metadata,
    relative_string,
};
use super::sources::{SourceScope, command_roots, mcp_roots, plugin_roots, skill_roots};

pub struct SkillCandidate {
    pub agent: Agent,
    pub name: String,
    pub name_key: String,
    pub version: Option<String>,
    pub source_root_scope: SourceScope,
    pub source_root_path: PathBuf,
    pub source_path: PathBuf,
    pub target_root: PathBuf,
    pub target_path: PathBuf,
}

pub struct CommandCandidate {
    pub agent: Agent,
    pub name: String,
    pub name_key: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub source_root_scope: SourceScope,
    pub source_root_path: PathBuf,
    pub source_path: PathBuf,
    pub source_relative_path: String,
    pub target_root: PathBuf,
    pub target_path: PathBuf,
}

pub struct PluginCandidate {
    pub agent: Agent,
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub source_root_scope: SourceScope,
    pub source_root_path: PathBuf,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
}

pub struct McpCandidate {
    pub agent: Agent,
    pub name: String,
    pub name_key: String,
    pub config: serde_json::Value,
    pub source_root_scope: SourceScope,
    pub source_root_path: PathBuf,
    pub source_path: String,
}

pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn exists_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok()
}

pub fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

pub fn target_root(
    home: &Path,
    workspace: Option<&Path>,
    kind: &str,
    scope: SourceScope,
) -> Option<PathBuf> {
    match scope {
        SourceScope::User => Some(home.join(".escode").join(kind)),
        SourceScope::Workspace => workspace.map(|workspace| workspace.join(".escode").join(kind)),
    }
}

pub fn config_path(home: &Path, workspace: Option<&Path>, scope: SourceScope) -> Option<PathBuf> {
    match scope {
        SourceScope::User => Some(home.join(".escode").join("cli").join("config.json")),
        SourceScope::Workspace => {
            workspace.map(|workspace| workspace.join(".escode").join("config.json"))
        }
    }
}

fn sorted(paths: impl IntoIterator<Item = PathBuf>, compare: &Compare) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = paths.into_iter().collect();
    paths.sort_by(|left, right| compare(&left.to_string_lossy(), &right.to_string_lossy()));
    paths
}

pub fn collect_skill_candidates(
    home: &Path,
    workspace: Option<&Path>,
    compare: &Compare,
) -> Vec<SkillCandidate> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for root in skill_roots(home, workspace) {
        if !exists_dir(&root.path) {
            continue;
        }
        let Some(target_root) = target_root(home, workspace, "skills", root.scope) else {
            continue;
        };
        for skill_path in sorted(walk_markdown_paths(&root.path), compare) {
            if !seen.insert(skill_path.clone()) {
                continue;
            }
            let Some(source_dir) = skill_path.parent() else {
                continue;
            };
            let metadata = read_skill_metadata(&skill_path);
            candidates.push(SkillCandidate {
                agent: root.agent,
                name_key: name_key(&metadata.name),
                name: metadata.name,
                version: metadata.version,
                source_root_scope: root.scope,
                source_root_path: root.path.clone(),
                source_path: source_dir.to_path_buf(),
                target_path: target_root.join(
                    source_dir
                        .file_name()
                        .map(std::ffi::OsStr::to_os_string)
                        .unwrap_or_default(),
                ),
                target_root: target_root.clone(),
            });
        }
    }
    candidates
}

pub fn collect_command_candidates(
    home: &Path,
    workspace: Option<&Path>,
    compare: &Compare,
) -> Vec<CommandCandidate> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for root in command_roots(home, workspace) {
        if !exists_dir(&root.path) {
            continue;
        }
        let Some(target_root) = target_root(home, workspace, "commands", root.scope) else {
            continue;
        };
        for command_path in sorted(collect_command_paths(&root.path), compare) {
            if !seen.insert(command_path.clone()) {
                continue;
            }
            let metadata = read_command_metadata(&command_path, &root.path);
            let source_relative_path = command_path
                .strip_prefix(&root.path)
                .map(relative_string)
                .unwrap_or_default();
            candidates.push(CommandCandidate {
                agent: root.agent,
                name_key: name_key(&metadata.name),
                name: metadata.name,
                description: metadata.description,
                argument_hint: metadata.argument_hint,
                source_root_scope: root.scope,
                source_root_path: root.path.clone(),
                target_path: join_relative(&target_root, &source_relative_path),
                source_path: command_path,
                source_relative_path,
                target_root: target_root.clone(),
            });
        }
    }
    candidates
}

pub fn collect_plugin_candidates(
    home: &Path,
    workspace: Option<&Path>,
    compare: &Compare,
) -> Vec<PluginCandidate> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for root in plugin_roots(home, workspace) {
        if !exists_dir(&root.path) {
            continue;
        }
        let Some(target_root) = target_root(home, workspace, "plugins", root.scope) else {
            continue;
        };
        for plugin_path in sorted(collect_plugin_paths(&root.path), compare) {
            if !seen.insert(plugin_path.clone()) {
                continue;
            }
            let Some(metadata) = read_plugin_metadata(&plugin_path) else {
                continue;
            };
            candidates.push(PluginCandidate {
                agent: root.agent,
                id: metadata.id,
                name: metadata.name,
                version: metadata.version,
                source_root_scope: root.scope,
                source_root_path: root.path.clone(),
                target_path: target_root.join(
                    plugin_path
                        .file_name()
                        .map(std::ffi::OsStr::to_os_string)
                        .unwrap_or_default(),
                ),
                source_path: plugin_path,
            });
        }
    }
    candidates
}

pub fn collect_mcp_candidates(home: &Path, workspace: Option<&Path>) -> Vec<McpCandidate> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for root in mcp_roots(home, workspace) {
        if !path_exists(&root.path) {
            continue;
        }
        for (name, config) in read_source_servers(&root.path, root.format) {
            let source_path = format!("{}#{name}", root.path.display());
            if !seen.insert(format!("{}:{source_path}", root.agent.as_str())) {
                continue;
            }
            candidates.push(McpCandidate {
                agent: root.agent,
                name_key: name_key(&name),
                config: strip_timeout_fields(&config),
                name,
                source_root_scope: root.scope,
                source_root_path: root.path.clone(),
                source_path,
            });
        }
    }
    candidates
}

/// TS `collectCommandMarkdownPaths`：递归、跳过点目录、无深度上限。
pub fn collect_command_paths(root: &Path) -> Vec<PathBuf> {
    let mut discovered = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && name.to_lowercase().ends_with(".md") {
                discovered.insert(path);
            }
        }
    }
    discovered.into_iter().collect()
}

/// TS `collectPluginPaths`：根下一层目录/软链，按 manifest 是否存在过滤。
pub fn collect_plugin_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut discovered = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if find_plugin_manifest(&path).is_some() {
            discovered.push(path);
        }
    }
    discovered
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in relative.split('/').filter(|segment| !segment.is_empty()) {
        path.push(segment);
    }
    path
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;
