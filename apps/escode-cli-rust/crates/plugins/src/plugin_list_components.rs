//! `plugins/list` 的组件枚举与计数（TS `enumeratePluginComponents` / skill 扫描 / 作者字段）。
#[allow(unused_imports)]
use super::plugin_list::*;

use super::{extension_config as config, extension_plugins as plugins, mcp_config};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// TS `enumeratePluginComponents`：分组顺序 agent → command → skill → hook → mcp，空组不出现。
/// 组件名/描述来自 loader 的枚举（与启用态无关）。
pub async fn components(plugin: &plugins::Plugin) -> Result<Value> {
    let mut groups: Vec<Value> = vec![];
    for (kind, field, default_dir) in [
        ("agent", "agents", "agents"),
        ("command", "commands", "commands"),
    ] {
        let items = markdown_components(plugin, field, default_dir).await?;
        if !items.is_empty() {
            groups.push(json!({"kind": kind, "items": items}));
        }
    }
    let skills = skill_components(plugin).await?;
    if !skills.is_empty() {
        groups.push(json!({"kind": "skill", "items": skills}));
    }
    // hook 组留到第 4 期：TS 用 loader 的 hook 源发现（manifest hooks + hook 文件，且要过
    // `canRunPluginHooks`）而不是直接读 manifest 键，本机 fixture 里 manifest 声明并不出现在
    // Node 的列表里。宁可少一组，也不输出 Node 不会显示的名字。
    let mcp: Vec<Value> = mcp_config::plugin_definitions(plugin)
        .await?
        .keys()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| json!({"name": name}))
        .collect();
    if !mcp.is_empty() {
        groups.push(json!({"kind": "mcp", "items": mcp}));
    }
    Ok(Value::Array(groups))
}

/// TS `collectMarkdownComponents`（agent / command）：先取 manifest 对象形式的声明（带描述），
/// 再扫默认目录与 manifest 声明的路径下的 `.md`，名字取 frontmatter `name` 或文件名，按名去重。
pub async fn markdown_components(
    plugin: &plugins::Plugin,
    field: &str,
    default_dir: &str,
) -> Result<Vec<Value>> {
    let mut items: Vec<Value> = vec![];
    let mut seen: Vec<String> = vec![];
    if let Some(declared) = plugin.manifest[field].as_object() {
        for (raw_name, meta) in declared {
            let name = raw_name.trim().to_owned();
            if name.is_empty() || seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let description = meta["description"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            items.push(match description {
                Some(description) => json!({"name": name, "description": description}),
                None => json!({"name": name}),
            });
        }
    }
    for dir in component_roots(plugin, default_dir) {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        let mut files: Vec<PathBuf> = vec![];
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().await?.is_file() && name.ends_with(".md") {
                files.push(entry.path());
            }
        }
        files.sort();
        for file in files {
            let fallback = file
                .file_name()
                .map(|name| name.to_string_lossy().trim_end_matches(".md").to_owned())
                .unwrap_or_default();
            let frontmatter = read_frontmatter(&file).await;
            let name = frontmatter.0.unwrap_or(fallback);
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            items.push(match frontmatter.1 {
                Some(description) => json!({"name": name, "description": description}),
                None => json!({"name": name}),
            });
        }
    }
    Ok(items)
}

/// TS `collectSkillComponents`：默认 `skills/` 与 manifest 声明路径下的 SKILL.md，
/// 名字取 frontmatter `name` 或技能目录名；按文件路径与最终名字双重去重。
pub async fn skill_components(plugin: &plugins::Plugin) -> Result<Vec<Value>> {
    let mut items: Vec<Value> = vec![];
    let mut seen_files: Vec<PathBuf> = vec![];
    let mut seen_names: Vec<String> = vec![];
    for dir in component_roots(plugin, "skills") {
        for file in skill_files_under(&dir).await? {
            if seen_files.contains(&file) {
                continue;
            }
            seen_files.push(file.clone());
            let fallback = file
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let frontmatter = read_frontmatter(&file).await;
            let name = frontmatter.0.unwrap_or(fallback);
            if seen_names.contains(&name) {
                continue;
            }
            seen_names.push(name.clone());
            items.push(match frontmatter.1 {
                Some(description) => json!({"name": name, "description": description}),
                None => json!({"name": name}),
            });
        }
    }
    Ok(items)
}

/// 两级扫描（根自身 + 一级子目录的真实 SKILL.md）；语义见 `plugin_skill_files`（TS 同序）。
pub async fn skill_files_under(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(super::plugin_skill_files::scan_skill_files_under(root).await)
}

/// TS `readMarkdownFrontmatter` 的 name/description 抽取（块标量交给现有 frontmatter 解析器，
/// 再按 TS `parseScalar` 去掉成对引号）。
pub async fn read_frontmatter(file: &Path) -> (Option<String>, Option<String>) {
    let Ok(content) = tokio::fs::read_to_string(file).await else {
        return (None, None);
    };
    let (present, fields, _) = escode_cli_domain::skills::frontmatter(&content);
    if !present {
        return (None, None);
    }
    let scalar = |key: &str| {
        fields
            .get(key)
            .map(|value| value.trim())
            .map(|value| {
                let quoted = (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''));
                if quoted && value.len() >= 2 {
                    value[1..value.len() - 1].trim().to_owned()
                } else {
                    value.to_owned()
                }
            })
            .filter(|value| !value.is_empty())
    };
    (scalar("name"), scalar("description"))
}

/// TS `toPluginInfo` 的 author/authorUrl 回退字段：manifest 里可以是字符串（名字）或
/// `{name, url}` 对象；其余形态忽略。
pub fn author_of(manifest: &Value) -> Option<(String, Option<String>)> {
    let author = manifest.get("author")?;
    if let Some(name) = author.as_str().map(str::trim).filter(|n| !n.is_empty()) {
        return Some((name.to_owned(), None));
    }
    let name = author["name"].as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let url = author["url"]
        .as_str()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);
    Some((name.to_owned(), url))
}

/// TS `resolveComponentRoots`：默认目录（存在才加）在前，manifest 声明的字符串/数组路径在后，
/// 按解析后的绝对路径去重；声明路径即使不存在也计入根数（与 TS 相同）。
pub fn component_roots(plugin: &plugins::Plugin, key: &str) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![];
    let default = plugin.root.join(key);
    if default.is_dir() && !roots.contains(&default) {
        roots.push(default);
    }
    for raw in config::strings(&plugin.manifest[key]) {
        let path = config::resolve(&plugin.root, raw.trim_start_matches("./"));
        if path.starts_with(&plugin.root) && !roots.contains(&path) {
            roots.push(path);
        }
    }
    roots
}

/// TS `materializeCommandMetadataRoot`：manifest `commands` 写成对象（内联命令元数据）时会额外
/// 生成一个命令根，计入 `commandRootCount`。这里只要能写出至少一条就按 1 计。
pub fn generated_command_root(plugin: &plugins::Plugin) -> usize {
    let Some(spec) = plugin.manifest["commands"].as_object() else {
        return 0;
    };
    spec.values()
        .any(|meta| {
            let source = meta["source"].is_string();
            let content = meta["content"].is_string();
            source ^ content
        })
        .into()
}

/// TS `countSkillFiles`：多根去重后的 SKILL.md 数量（扫描语义见 `plugin_skill_files`）。
pub async fn count_skill_files(roots: &[PathBuf]) -> Result<usize> {
    let mut files: Vec<PathBuf> = Vec::new();
    for root in roots {
        for file in super::plugin_skill_files::scan_skill_files_under(root).await {
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    Ok(files.len())
}

/// 保留导出：`plugin_list` 等调用方经此访问共享判定（单点仍在 `plugin_skill_files`）。
pub use super::plugin_skill_files::should_walk;
