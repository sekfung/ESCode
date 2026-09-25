//! 自定义 slash 命令发现与加载（docs/specs/rust-custom-commands.md），对齐 TS
//! `adapters/src/commands/{index,roots}.ts` 与插件 `resolveCommandRoots`。
use super::{extension_config as config, extension_plugins as plugins};
use crate::domain::custom_command::{self as cc, Metadata, PluginContext};
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub(crate) struct Root {
    pub path: PathBuf,
    pub scope: String,
    pub source: String,
    pub plugin: Option<PluginContext>,
}
#[derive(Clone)]
pub(crate) struct Command {
    pub meta: Metadata,
    pub path: PathBuf,
    pub root: PathBuf,
    pub plugin: Option<PluginContext>,
}

fn root(path: PathBuf, scope: &str, source: &str, plugin: Option<PluginContext>) -> Root {
    Root {
        path,
        scope: scope.into(),
        source: source.into(),
        plugin,
    }
}
/// user 根 → cwd 到 git 根每一级（cwd 优先）→ 插件根（TS 优先级 10 步进，插件从 1000 起）。
pub(crate) async fn default_roots(home: &Path, cwd: &Path) -> Vec<Root> {
    let mut roots = vec![];
    for (base, scope) in std::iter::once((home.to_owned(), "user")).chain(
        config::project_directories(cwd)
            .await
            .into_iter()
            .map(|d| (d, "project")),
    ) {
        roots.push(root(
            base.join(".zcode").join("commands"),
            scope,
            "zcode",
            None,
        ));
        roots.push(root(
            base.join(".agents").join("commands"),
            scope,
            "agents",
            None,
        ));
    }
    roots
}
fn resolve_inside(base: &Path, raw: &str) -> Option<PathBuf> {
    if Path::new(raw).is_absolute() || Path::new(raw).has_root() {
        return None;
    }
    let mut result = base.to_owned();
    for part in Path::new(raw).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() || !result.starts_with(base) {
                    return None;
                }
            }
            Component::Normal(p) => result.push(p),
            _ => return None,
        }
    }
    result.starts_with(base).then_some(result)
}
async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok_and(|m| m.is_dir())
}
async fn plugin_roots(
    cwd: &Path,
    settings: &Value,
    cancel: &CancellationToken,
) -> Result<Vec<Root>> {
    let data_root = config::storage(settings).join("data");
    let mut roots = vec![];
    for plugin in plugins::enabled(cwd, settings, cancel).await? {
        let data = data_root.join(cc::sanitize_plugin_id(&plugin.id));
        let context = PluginContext {
            data_path: data.to_string_lossy().into_owned(),
            id: plugin.id.clone(),
            name: plugin.name.clone(),
            root_path: plugin.root.to_string_lossy().into_owned(),
        };
        let scope = if plugin.official { "system" } else { "user" };
        let mut paths: Vec<String> = config::strings(&plugin.manifest["commands"])
            .into_iter()
            .map(str::to_owned)
            .collect();
        if is_dir(&plugin.root.join("commands")).await {
            paths.insert(0, "commands".into());
        }
        let mut seen = BTreeSet::new();
        for raw in paths {
            if let Some(path) = resolve_inside(&plugin.root, &raw)
                && seen.insert(path.clone())
            {
                roots.push(root(path, scope, "plugin", Some(context.clone())));
            }
        }
        if let Some(spec) = plugin.manifest["commands"].as_object()
            && generate(&plugin.root, &data.join("generated-commands"), spec).await?
        {
            roots.push(root(
                data.join("generated-commands"),
                scope,
                "plugin",
                Some(context),
            ));
        }
    }
    Ok(roots)
}
/// manifest `commands` 对象映射落成 markdown（TS materializeCommandMetadataRoot）；非法条目跳过。
async fn generate(
    plugin_root: &Path,
    target: &Path,
    spec: &serde_json::Map<String, Value>,
) -> Result<bool> {
    tokio::fs::create_dir_all(target).await?;
    let mut wrote = false;
    for (raw_name, metadata) in spec {
        let (Some(name), true) = (cc::generated_command_name(raw_name), metadata.is_object())
        else {
            continue;
        };
        let markdown = match (metadata["source"].as_str(), metadata["content"].as_str()) {
            (Some(source), None) => {
                let Some(path) =
                    resolve_inside(plugin_root, source.strip_prefix("./").unwrap_or(source))
                else {
                    continue;
                };
                match tokio::fs::read(&path).await {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(_) => continue,
                }
            }
            (None, Some(content)) => content.to_owned(),
            _ => continue,
        };
        tokio::fs::write(
            target.join(format!("{name}.md")),
            cc::generated_command_markdown(&markdown, metadata),
        )
        .await?;
        wrote = true;
    }
    Ok(wrote)
}
async fn scan(dir: PathBuf, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > cc::MAX_SCAN_DEPTH {
        return;
    }
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return;
    };
    let mut children = vec![];
    while let Ok(Some(entry)) = entries.next_entry().await {
        children.push(entry.path());
    }
    children.sort();
    for path in children {
        // metadata 跟随 symlink；悬空链接与无权限条目跳过。
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if meta.is_dir() {
            Box::pin(scan(path, depth + 1, out)).await;
        } else if meta.is_file()
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".md"))
        {
            out.push(path);
        }
    }
}
fn command_name(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let relative = &relative[..relative.len() - 3];
    cc::normalize_name(
        &relative
            .split(['/', '\\'])
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(":"),
    )
}
/// 按优先级扫描并先到先赢，最终按名称排序（TS discoverCommands）。
pub(crate) async fn discover_in(
    roots: &[Root],
    disabled: &BTreeSet<PathBuf>,
    cancel: &CancellationToken,
) -> Result<Vec<Command>> {
    let mut selected: BTreeMap<String, Command> = BTreeMap::new();
    for root in roots {
        super::tools::check_cancel(cancel)?;
        if !is_dir(&root.path).await {
            continue;
        }
        let mut files = vec![];
        scan(root.path.clone(), 0, &mut files).await;
        for path in files {
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let raw = String::from_utf8_lossy(&bytes);
            let name = command_name(&path, &root.path);
            let Some(meta) = cc::parse_command(&raw, &name, &root.scope, &root.source) else {
                continue;
            };
            if disabled.contains(&path) {
                continue;
            }
            selected.entry(meta.name.clone()).or_insert(Command {
                meta,
                path,
                root: root.path.clone(),
                plugin: root.plugin.clone(),
            });
        }
    }
    let mut commands: Vec<Command> = selected.into_values().collect();
    commands.sort_by(|a, b| cc::compare_names(&a.meta.name, &b.meta.name));
    Ok(commands)
}
/// 正文：读取前 100000 字节，去掉 frontmatter 后 trim（TS loadCommand）。
pub(crate) async fn content(command: &Command) -> Result<String> {
    let mut bytes = vec![];
    tokio::fs::File::open(&command.path)
        .await?
        .take(cc::MAX_COMMAND_BYTES as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(cc::command_content(&String::from_utf8_lossy(&bytes)))
}
/// 会话 workspace 的完整命令集合：config 禁用项（`command.<path>.enable=false`）已剔除。
pub(crate) async fn discover(cwd: &Path, cancel: &CancellationToken) -> Result<Vec<Command>> {
    let settings = config::load(cwd).await?;
    let mut roots = default_roots(&config::home(), cwd).await;
    roots.extend(plugin_roots(cwd, &settings, cancel).await?);
    let disabled = settings["command"]
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .filter(|(_, v)| v["enable"] == false)
                .map(|(path, _)| config::resolve(cwd, path))
                .collect()
        })
        .unwrap_or_default();
    discover_in(&roots, &disabled, cancel).await
}
