//! `plugins/describe` 的组件扫描（TS `enumeratePluginComponents` / `listPluginHookEventNames`）：只读文件，不跟随符号链接。
#[allow(unused_imports)]
use super::plugin_describe::*;

use super::plugin_install as install;
use super::plugin_validate::diag;
use crate::domain::json_order::Json;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

pub(super) fn item(name: String, description: Option<String>) -> Value {
    match description {
        Some(description) => json!({ "name": name, "description": description }),
        None => json!({ "name": name }),
    }
}

/// TS `collectComponentDirs`：默认目录（存在时）+ manifest 字符串 / 数组路径，按解析结果去重。
pub(super) fn component_dirs(root: &Path, field: Option<&Json>, default_dir: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![];
    let mut add = |raw: &str| {
        if let Some(path) = install::resolve_inside(root, raw.strip_prefix("./").unwrap_or(raw))
            && !dirs.contains(&path)
        {
            dirs.push(path);
        }
    };
    if root.join(default_dir).is_dir() {
        add(default_dir);
    }
    match field {
        Some(Json::String(path)) => add(path),
        Some(Json::Array(items)) => items.iter().filter_map(Json::as_str).for_each(add),
        _ => {}
    }
    dirs
}

/// 目录项（不跟随符号链接），顺序与 Node `readdirSync` 一致：Unix 上 libuv 的 scandir 按名字字节序（strcmp）
/// 排序，而 `read_dir` 给的是文件系统原始顺序（ext4 为哈希序）；Windows 上两者都是 NTFS 返回的顺序，不再排序。
pub(super) fn entries(dir: &Path) -> Vec<(String, std::fs::FileType)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return vec![];
    };
    #[cfg_attr(windows, allow(unused_mut))]
    let mut items: Vec<_> = read
        .flatten()
        .filter_map(|entry| {
            Some((
                entry.file_name().into_string().ok()?,
                entry.file_type().ok()?,
            ))
        })
        .collect();
    #[cfg(not(windows))]
    items.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    items
}

/// TS `collectMarkdownComponents`：对象形式声明在前，再扫目录里的 `.md`（frontmatter name / description）。
pub(super) fn markdown_components(
    root: &Path,
    field: Option<&Json>,
    default_dir: &str,
) -> Vec<Value> {
    let mut items = vec![];
    let mut seen = HashSet::new();
    if let Some(Json::Object(declared)) = field {
        for (raw_name, meta) in declared {
            let name = raw_name.trim().to_owned();
            if name.is_empty() || !seen.insert(name.clone()) {
                continue;
            }
            let description = meta
                .get("description")
                .and_then(Json::as_str)
                .map(|d| d.trim().to_owned())
                .filter(|d| !d.is_empty());
            items.push(item(name, description));
        }
    }
    for dir in component_dirs(root, field, default_dir) {
        for (file_name, kind) in entries(&dir) {
            let Some(base) = file_name.strip_suffix(".md").filter(|_| kind.is_file()) else {
                continue;
            };
            let (name, description) = read_frontmatter(&dir.join(&file_name));
            let name = name.unwrap_or_else(|| base.to_owned());
            if seen.insert(name.clone()) {
                items.push(item(name, description));
            }
        }
    }
    items
}

/// TS `collectSkillComponents` + `scanSkillFilesUnderRootSync`（不跟随符号链接）。
pub(super) fn skill_components(root: &Path, field: Option<&Json>) -> Vec<Value> {
    let mut items = vec![];
    let mut seen_files = HashSet::new();
    let mut seen_names = HashSet::new();
    let is_real_file =
        |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file());
    for dir in component_dirs(root, field, "skills") {
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let mut files = vec![];
        let own = dir.join("SKILL.md");
        if is_real_file(&own) {
            files.push(own);
        }
        for (name, kind) in entries(&dir) {
            if !kind.is_dir() || !super::plugin_list::should_walk(&name) {
                continue;
            }
            let candidate = dir.join(&name).join("SKILL.md");
            if is_real_file(&candidate) {
                files.push(candidate);
            }
        }
        for file in files {
            if !seen_files.insert(file.clone()) {
                continue;
            }
            let fallback = file
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (name, description) = read_frontmatter(&file);
            let name = name.unwrap_or(fallback);
            if seen_names.insert(name.clone()) {
                items.push(item(name, description));
            }
        }
    }
    items
}

/// TS `listPluginHookSources` + `listPluginHookEventNames`：`hooks/hooks.json` 与 manifest `hooks`
/// （路径 / 内联对象 / 数组）只取事件名；不支持的事件与读取问题记诊断。
pub(super) fn hook_events(
    root: &Path,
    manifest: &Json,
    id: &str,
    out: &mut Vec<Value>,
) -> Vec<String> {
    let realpath = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    // (原始 hooks, 是否文件包装)
    let mut sources: Vec<(Option<Json>, bool)> = vec![];
    let mut loaded_paths: Vec<PathBuf> = vec![];
    let load = |path: &Path, out: &mut Vec<Value>| -> Option<Json> {
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| Json::parse(&text));
        if parsed.is_none() {
            out.push(diag(
                "plugin_hook_read_failed",
                format!("Unexpected token in JSON at {}", path.display()),
                Some(id),
                true,
            ));
        }
        parsed
    };
    let standard = root.join("hooks").join("hooks.json");
    if standard.is_file()
        && let Some(raw) = load(&standard, out)
    {
        sources.push((Some(raw), true));
        loaded_paths.push(realpath(&standard));
    }
    if let Some(hooks) = manifest.get("hooks") {
        let specs: Vec<&Json> = match hooks {
            Json::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for spec in specs {
            let Json::String(spec) = spec else {
                sources.push((Some(spec.clone()), false));
                continue;
            };
            let Some(path) = install::resolve_inside(root, spec) else {
                out.push(diag(
                    "plugin_component_path_invalid",
                    format!("Plugin hooks path escapes plugin root: {spec}"),
                    Some(id),
                    true,
                ));
                continue;
            };
            if !path.is_file() {
                out.push(diag(
                    "plugin_hook_read_failed",
                    format!("Plugin hooks file not found: {spec}"),
                    Some(id),
                    true,
                ));
                continue;
            }
            let real = realpath(&path);
            if loaded_paths.contains(&real) {
                out.push(diag(
                    "plugin_hook_invalid",
                    format!("Duplicate plugin hooks file ignored: {spec}"),
                    Some(id),
                    false,
                ));
                continue;
            }
            if let Some(raw) = load(&path, out) {
                sources.push((Some(raw), true));
                loaded_paths.push(real);
            }
        }
    }
    let mut names: Vec<String> = vec![];
    for (raw, wrapper) in sources {
        let hooks_root = if wrapper {
            raw.as_ref()
                .filter(|r| r.is_object())
                .and_then(|r| r.get("hooks").cloned())
        } else {
            raw
        };
        let Some(Json::Object(events)) = hooks_root else {
            out.push(diag(
                "plugin_hook_invalid",
                if wrapper {
                    "Plugin hooks file must contain a hooks object"
                } else {
                    "Plugin manifest hooks entry must be an object, a path, or an array"
                },
                Some(id),
                true,
            ));
            continue;
        };
        for (event, _) in events {
            if !HOOK_EVENTS.contains(&event.as_str()) {
                out.push(diag(
                    "plugin_hook_unsupported_event",
                    format!("Plugin hook event is not supported by this ZCode runtime: {event}"),
                    Some(id),
                    false,
                ));
                continue;
            }
            if !names.contains(&event) {
                names.push(event);
            }
        }
    }
    names
}
