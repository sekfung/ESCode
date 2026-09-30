//! `plugins/list` 读面（docs/specs/rust-plugins.md 第 1 期）：
//! 对齐 TS `bootstrap/src/zcode-protocol/plugins.ts::listPlugins` + `toPluginInfo` +
//! `createMissingConfiguredPluginInfos`。第 1 期只输出「发现层 + 清单 + 计数 + MCP 名」，
//! 选项面（userConfig/configuredOptions/optionSources）、hookDetails、enabledSource/rootSource
//! 与 components 分组留给后续期（schema 里都是可选字段，缺失好过伪造）。

use super::{extension_config as config, extension_plugins as plugins, mcp_config};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub(super) async fn list(cwd: &Path, config: &Value, cancel: &CancellationToken) -> Result<Value> {
    let data_root = config::storage(config).join("data");
    let mut items: Vec<Value> = vec![];
    let mut seen: Vec<String> = vec![];
    for plugin in plugins::all(cwd, config, cancel).await? {
        seen.push(plugin.id.clone());
        let mut info = json!({
            "id": plugin.id,
            "name": plugin.name,
            "enabled": plugin.enabled,
            "source": plugin.source,
            "marketplace": plugin.marketplace,
            "rootPath": plugin.root.to_string_lossy(),
            // TS `toPluginInfo` 始终带 skillCount：停用插件走 emptyComponents 得 0。
            "skillCount": 0,
            "skillRootCount": 0,
            "commandRootCount": 0,
            "mcpServerNames": [],
        });
        let manifest = &plugin.manifest;
        if let Some(description) = manifest["description"].as_str() {
            info["description"] = description.into();
        }
        if let Some(version) = manifest["version"].as_str() {
            info["version"] = version.into();
        }
        if let Some((author, url)) = author_of(manifest) {
            info["author"] = author.into();
            if let Some(url) = url {
                info["authorUrl"] = url.into();
            }
        }
        if let Some(homepage) = manifest["homepage"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
        {
            info["homepage"] = homepage.into();
        }
        let declared: Vec<Value> = mcp_config::plugin_definitions(&plugin)
            .await?
            .keys()
            .map(|name| Value::from(name.clone()))
            .collect();
        info["declaredMcpServerNames"] = Value::Array(declared);
        if plugin.enabled {
            // 与 TS 一致：只有启用插件才解析真实组件（停用插件计数为 0、MCP 名为空），
            // 但 `components` 清单在后续期里与启用态无关。
            let skill_roots = component_roots(&plugin, "skills");
            let command_roots = component_roots(&plugin, "commands");
            info["skillRootCount"] = json!(skill_roots.len());
            info["commandRootCount"] = json!(command_roots.len() + generated_command_root(&plugin));
            info["skillCount"] = json!(count_skill_files(&skill_roots).await?);
            let mut names: Vec<Value> = vec![];
            for (name, server) in mcp_config::plugin_servers(&plugin, cwd, &data_root).await? {
                if !server.invalid {
                    names.push(name.into());
                }
            }
            info["mcpServerNames"] = Value::Array(names);
        }
        items.push(info);
    }
    items.extend(missing(&config["plugins"], &seen));
    Ok(json!({ "plugins": items }))
}

/// TS `toPluginInfo` 的 author/authorUrl 回退字段：manifest 里可以是字符串（名字）或
/// `{name, url}` 对象；其余形态忽略。
fn author_of(manifest: &Value) -> Option<(String, Option<String>)> {
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
fn component_roots(plugin: &plugins::Plugin, key: &str) -> Vec<PathBuf> {
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
fn generated_command_root(plugin: &plugins::Plugin) -> usize {
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

/// TS `countSkillFiles` + `scanSkillFilesUnderRootSync(followSymbolicLinks: false)`：
/// 根自身的 SKILL.md 与一级子目录的 SKILL.md（真实文件，不跟随符号链接）。
async fn count_skill_files(roots: &[PathBuf]) -> Result<usize> {
    let mut files: Vec<PathBuf> = vec![];
    for root in roots {
        if tokio::fs::symlink_metadata(root)
            .await
            .is_ok_and(|m| m.file_type().is_symlink())
            || !tokio::fs::metadata(root).await.is_ok_and(|m| m.is_dir())
        {
            continue;
        }
        let mut candidates = vec![root.join("SKILL.md")];
        if let Ok(mut entries) = tokio::fs::read_dir(root).await {
            while let Some(entry) = entries.next_entry().await? {
                if !entry.file_type().await?.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !should_walk(&name) {
                    continue;
                }
                candidates.push(entry.path().join("SKILL.md"));
            }
        }
        for candidate in candidates {
            if files.contains(&candidate) {
                continue;
            }
            if tokio::fs::symlink_metadata(&candidate)
                .await
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                continue;
            }
            if tokio::fs::metadata(&candidate)
                .await
                .is_ok_and(|m| m.is_file())
            {
                files.push(candidate);
            }
        }
    }
    Ok(files.len())
}

/// 与 TS `shouldWalkSkillDirectoryEntry` 同义：跳过隐藏目录与常见构建/依赖目录。
fn should_walk(name: &str) -> bool {
    if name.starts_with('.') && name != ".system" {
        return false;
    }
    ![
        "node_modules",
        "dist",
        "build",
        "out",
        "target",
        "vendor",
        "coverage",
        "__pycache__",
    ]
    .contains(&name)
}

/// TS `createMissingConfiguredPluginInfos`：配置里声明过、但本次没有发现的插件补一行
/// `source: "missing"` + `packageStatus: "missing"`，避免「开关还在、列表里却消失了」。
fn missing(config: &Value, discovered: &[String]) -> Vec<Value> {
    let plugins = &config;
    let mut ids: Vec<String> = vec![];
    for key in ["enabledPlugins", "options"] {
        if let Some(entries) = plugins[key].as_object() {
            for id in entries.keys() {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
    }
    let mut rows = vec![];
    for id in ids {
        if discovered.contains(&id) {
            continue;
        }
        let Some((name, marketplace)) = id.rsplit_once('@') else {
            continue;
        };
        if name.is_empty() || marketplace.is_empty() {
            continue;
        }
        let enabled = plugins["enabledPlugins"][&id].as_bool().unwrap_or(false);
        rows.push(json!({
            "id": id,
            "name": name,
            "enabled": enabled,
            "source": "missing",
            "marketplace": marketplace,
            "skillCount": 0,
            "skillRootCount": 0,
            "commandRootCount": 0,
            "components": [],
            "declaredMcpServerNames": [],
            "mcpServerNames": [],
            "rootPath": "",
            "packageStatus": "missing",
        }));
    }
    rows
}

/// `plugins/list` 的入口参数（TS `zcodePluginsListParamsSchema`）：workspace 必填、configScope 可选。
pub(super) fn workspace_path(params: &Value) -> Result<PathBuf> {
    Ok(PathBuf::from(
        params["workspace"]["workspacePath"]
            .as_str()
            .context("Workspace path required")?,
    ))
}

/// 供 `Tools::plugin_list` 使用的配置视图：`configScope: "user"` 时不加载项目层配置。
pub(super) async fn config_for(cwd: &Path, params: &Value) -> Result<Value> {
    if params["configScope"].as_str() == Some("user") {
        config::load_user().await
    } else {
        config::load(cwd).await
    }
}
