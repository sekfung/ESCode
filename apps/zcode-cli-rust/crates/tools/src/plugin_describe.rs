//! `plugins/describe`（docs/specs/rust-plugin-marketplace-write.md W5b）：对齐 TS `describePlugin` →
//! adapters `describeMarketplacePlugin` / `readComponentsAtRoot` / `enumeratePluginComponents`。
//! 已安装插件读本地安装目录；未安装候选按源物化到临时目录（用完即删）后枚举组件名称与描述。

use super::plugin_install as install;
use super::plugin_marketplace as market;
use super::plugin_uninstall::{read_installed_sync, storage_lock};
use super::plugin_validate::{
    diag, entry_name, error_diagnostic, mcp_definitions, read_root_manifest,
};
use super::{extension_config as config, plugin_list};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
];

pub(super) async fn describe(params: &Value) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let name = plugin_list::non_empty(params, "pluginName")?.to_owned();
    let marketplace = plugin_list::non_empty(params, "marketplace")?.to_owned();
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    market::ensure_default_marketplaces(&storage)?;
    let id = format!("{name}@{marketplace}");

    // 已安装优先：本地目录无需联网；记录在但目录缺失时继续走源解析兜底。
    let installed = read_installed_sync(&storage).into_iter().find(|record| {
        let field = |key: &str| record.get(key).and_then(Json::as_str);
        field("marketplace") == Some(marketplace.as_str()) && field("name") == Some(name.as_str())
    });
    if let Some(record) = installed {
        let root = installed_root(&storage, &record);
        if root.is_dir() {
            let mut diagnostics = vec![];
            let read = read_components(&root, None, &marketplace, &mut diagnostics);
            return Ok(result(read, diagnostics));
        }
    }

    if let Err(error) = super::plugin_market_write::ensure_manifest(&storage, &marketplace).await {
        return Ok(result(
            (vec![], None),
            vec![error_diagnostic(&error, Some(&id))],
        ));
    }
    if market::manifest(&storage, &marketplace).is_none() {
        let message = format!("Marketplace not found: {marketplace}");
        return Ok(result(
            (vec![], None),
            vec![diag("plugin_marketplace_invalid", message, None, true)],
        ));
    }
    let Some(entry) = install::ordered_entry(&storage, &marketplace, &name) else {
        let message = format!("Plugin not found: {id}");
        return Ok(result(
            (vec![], None),
            vec![diag("plugin_not_found", message, None, true)],
        ));
    };
    tokio::task::spawn_blocking(move || {
        let mut diagnostics = vec![];
        match install::materialize(&storage, &marketplace, &entry, None) {
            Ok(root) => {
                let read =
                    read_components(&root.path, Some(&entry), &marketplace, &mut diagnostics);
                root.cleanup();
                result(read, diagnostics)
            }
            Err(error) => result((vec![], None), vec![error_diagnostic(&error, Some(&id))]),
        }
    })
    .await
    .map_err(|_| anyhow!("Plugin describe worker panicked"))
}

type Components = (Vec<Value>, Option<Value>);

fn result((components, metadata): Components, diagnostics: Vec<Value>) -> Value {
    let mut out = json!({ "components": components });
    if !diagnostics.is_empty() {
        out["diagnostics"] = Value::Array(diagnostics);
    }
    if let Some(metadata) = metadata {
        out["metadata"] = metadata;
    }
    out
}

/// TS `resolveInstalledPluginRoot`：记录的 installPath，缺省按缓存目录约定；恢复中断的原子激活。
fn installed_root(storage: &Path, record: &Json) -> PathBuf {
    let field = |key: &str| {
        record
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let path = field("installPath");
    let root = if path.is_empty() {
        storage
            .join("cache")
            .join(market::sanitize(&field("marketplace")))
            .join(market::sanitize(&field("name")))
            .join(market::sanitize(&field("version")))
    } else {
        PathBuf::from(path)
    };
    super::atomic_dir::recover(&root)
}

/// TS `readComponentsAtRoot`：manifest 读取失败按 null 降级，仍按默认目录约定扫描。
fn read_components(
    root: &Path,
    entry: Option<&Json>,
    marketplace: &str,
    diagnostics: &mut Vec<Value>,
) -> Components {
    let placeholder = Json::parse(r#"{"name":"__describe__"}"#).unwrap_or_else(Json::object);
    let manifest = read_root_manifest(root, entry.unwrap_or(&placeholder))
        .ok()
        .flatten();
    let field = |key: &str| manifest.as_ref().and_then(|m| m.get(key));
    let mut groups = vec![];
    let mut push = |kind: &str, items: Vec<Value>| {
        if !items.is_empty() {
            groups.push(json!({ "kind": kind, "items": items }));
        }
    };
    push(
        "agent",
        markdown_components(root, field("agents"), "agents"),
    );
    push(
        "command",
        markdown_components(root, field("commands"), "commands"),
    );
    push("skill", skill_components(root, field("skills")));
    if let Some(manifest) = &manifest {
        let id = format!("{}@{marketplace}", entry_name(manifest));
        let hooks = hook_events(root, manifest, &id, diagnostics);
        push(
            "hook",
            hooks
                .into_iter()
                .map(|name| json!({ "name": name }))
                .collect(),
        );
        let servers = mcp_definitions(root, manifest, &id, diagnostics);
        push(
            "mcp",
            servers
                .into_iter()
                .map(|(name, _)| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .map(|name| json!({ "name": name }))
                .collect(),
        );
    }
    let metadata = manifest.as_ref().and_then(display_metadata);
    (groups, metadata)
}

fn item(name: String, description: Option<String>) -> Value {
    match description {
        Some(description) => json!({ "name": name, "description": description }),
        None => json!({ "name": name }),
    }
}

/// TS `collectComponentDirs`：默认目录（存在时）+ manifest 字符串 / 数组路径，按解析结果去重。
fn component_dirs(root: &Path, field: Option<&Json>, default_dir: &str) -> Vec<PathBuf> {
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

/// 目录项（不跟随符号链接；按 OS 顺序，与 Node `readdirSync` 一致）。
fn entries(dir: &Path) -> Vec<(String, std::fs::FileType)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return vec![];
    };
    read.flatten()
        .filter_map(|entry| {
            Some((
                entry.file_name().into_string().ok()?,
                entry.file_type().ok()?,
            ))
        })
        .collect()
}

/// TS `collectMarkdownComponents`：对象形式声明在前，再扫目录里的 `.md`（frontmatter name / description）。
fn markdown_components(root: &Path, field: Option<&Json>, default_dir: &str) -> Vec<Value> {
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
fn skill_components(root: &Path, field: Option<&Json>) -> Vec<Value> {
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
fn hook_events(root: &Path, manifest: &Json, id: &str, out: &mut Vec<Value>) -> Vec<String> {
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

/// TS `toManifestDisplayMetadata` + `normalizeAuthorValue`。
fn display_metadata(manifest: &Json) -> Option<Value> {
    let trimmed = |value: Option<&Json>| {
        value
            .and_then(Json::as_str)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (author, author_url) = match manifest.get("author") {
        Some(Json::String(name)) => (trimmed(Some(&Json::str(name.clone()))), None),
        Some(author @ Json::Object(_)) => (trimmed(author.get("name")), trimmed(author.get("url"))),
        _ => (None, None),
    };
    let mut out = serde_json::Map::new();
    if let Some(author) = author {
        out.insert("author".into(), author.into());
    }
    if let Some(url) = author_url {
        out.insert("authorUrl".into(), url.into());
    }
    if let Some(homepage) = manifest
        .get("homepage")
        .and_then(Json::as_str)
        .filter(|h| !h.trim().is_empty())
    {
        out.insert("homepage".into(), homepage.into());
    }
    if let Some(version) = manifest
        .get("version")
        .and_then(Json::as_str)
        .filter(|v| !v.is_empty())
    {
        out.insert("version".into(), version.into());
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

// ---- Markdown frontmatter（TS plugins/markdown-frontmatter.ts：只取 name / description，支持块标量） ----

fn read_frontmatter(path: &Path) -> (Option<String>, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_frontmatter(&content),
        Err(_) => (None, None),
    }
}

fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    if !content.starts_with("---") {
        return (None, None);
    }
    let lines: Vec<&str> = content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    if lines[0].trim() != "---" {
        return (None, None);
    }
    let Some(end) = lines
        .iter()
        .skip(1)
        .position(|l| l.trim() == "---")
        .map(|i| i + 1)
    else {
        return (None, None);
    };
    let values = flat_scalars(&lines[1..end]);
    let get = |key: &str| {
        values
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| scalar(v))
    };
    (get("name"), get("description"))
}

fn indented(line: &str) -> bool {
    line.starts_with(char::is_whitespace)
}

fn flat_scalars(lines: &[&str]) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = vec![];
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() || line.trim().starts_with('#') || indented(line) {
            continue;
        }
        let Some(separator) = line.find(':').filter(|s| *s > 0) else {
            continue;
        };
        let key = line[..separator].trim().to_owned();
        let value = line[separator + 1..].trim();
        if values.iter().any(|(k, _)| *k == key) {
            continue;
        }
        let style = match value {
            ">" | ">+" | ">-" => Some(true),
            "|" | "|+" | "|-" => Some(false),
            _ => None,
        };
        let Some(folded) = style else {
            values.push((key, value.to_owned()));
            continue;
        };
        let start = index;
        while index < lines.len() && (lines[index].trim().is_empty() || indented(lines[index])) {
            index += 1;
        }
        let raw = &lines[start..index];
        let indent = raw
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let content: Vec<&str> = raw
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    ""
                } else {
                    &l[indent..]
                }
            })
            .collect();
        let value = if folded {
            let mut paragraphs: Vec<String> = vec![];
            let mut current: Vec<&str> = vec![];
            for line in content {
                if line.trim().is_empty() {
                    if !current.is_empty() {
                        paragraphs.push(current.join(" "));
                        current.clear();
                    }
                } else {
                    current.push(line.trim());
                }
            }
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
            }
            paragraphs.join("\n").trim().to_owned()
        } else {
            content.join("\n").trim().to_owned()
        };
        values.push((key, value));
    }
    values
}

fn scalar(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let quoted = trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')));
    let value = if quoted {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_block_scalars() {
        let text = "---\nname: \"demo\"\ndescription: >\n  first line\n  second\n\n  next para\nother: x\n---\nbody";
        assert_eq!(
            parse_frontmatter(text),
            (
                Some("demo".into()),
                Some("first line second\nnext para".into())
            )
        );
        let literal = "---\r\ndescription: |\r\n  a\r\n    b\r\n---\r\n";
        assert_eq!(parse_frontmatter(literal), (None, Some("a\n  b".into())));
        assert_eq!(parse_frontmatter("no frontmatter"), (None, None));
        assert_eq!(parse_frontmatter("---\nname: ''\n---"), (None, None));
    }
}
