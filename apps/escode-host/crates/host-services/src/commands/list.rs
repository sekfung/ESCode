//! `commands.list`：多来源命令发现（TS `commandsService.ts:520-573` + 两个递归发现器）。
//!
//! 两条顺序约束（都是可见行为，不能重排）：
//! 1. **先 workspace 再 user**，且按 agentSource 分组——每个 agentSource 内先把全部 workspace 目录
//!    合并、再合并全部 user 目录；按目录交错读会让 user `.escode` 抢在 workspace `.agents` 前面；
//! 2. **`.escode` 是强优先级来源**：同 scope 下只要 `.escode` 读到有效命令，`.agents` 就不再参与。
//!
//! 重名去重保留**先出现**的那条（`dedupeCommandsByName` 的 Map 语义）。
//! 插件命令按 `name` 的 `localeCompare` 排序（比较器由 RPC 层注入 ICU 实现）。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::config::{enabled_overrides, is_enabled, read_config};
use super::descriptors::{SourceDescriptor, descriptor_for, directory_sources};
use super::parser::{parse_command_file, parsed_fields};
use super::plugins::{RootDescriptor, resolve_root_descriptors};
use super::write::{command_name, location};

pub struct ListParams<'a> {
    pub agent_source: Option<&'a str>,
    pub workspace_path: Option<&'a str>,
}

type Compare = dyn Fn(&str, &str) -> Ordering + Send + Sync;

pub async fn list(home: &Path, params: ListParams<'_>, compare: &Compare) -> Result<Value, String> {
    let config = read_config(home).await;
    let overrides = enabled_overrides(&config);
    let requested = params.agent_source;
    // 未知来源在这里就失败（与目录解析一致）。
    descriptor_for(requested)?;
    let descriptors: &[SourceDescriptor] =
        if requested.is_none() || requested == Some("escodeAgent") {
            directory_sources()
        } else {
            &[]
        };

    let mut user_commands: Vec<Value> = Vec::new();
    if let Some(workspace) = params.workspace_path.filter(|path| !path.is_empty()) {
        let project_root = |descriptor: &SourceDescriptor| {
            let mut root = PathBuf::from(workspace);
            for segment in descriptor.workspace_directory_segments {
                root.push(segment);
            }
            root
        };
        discover_from_sources(
            descriptors,
            project_root,
            "project",
            &mut user_commands,
            &overrides,
            Some(workspace),
        )
        .await;
    }
    discover_from_sources(
        descriptors,
        |descriptor| descriptor.user_directory(home),
        "global",
        &mut user_commands,
        &overrides,
        None,
    )
    .await;

    let user_commands = dedupe_by_name(user_commands);
    let plugin_commands = if requested.is_none() || requested == Some("escodeAgent") {
        discover_plugin_commands(home, &config, &overrides, compare).await
    } else {
        Vec::new()
    };

    let mut commands = user_commands.clone();
    commands.extend(plugin_commands.iter().cloned());
    Ok(json!({
        "commands": commands,
        "userCommands": user_commands,
        "pluginCommands": plugin_commands,
        "capability": {"userScopeAvailable": true},
    }))
}

/// 同一 agentSource 的目录来源表：`.escode` 在前、`.agents` 在后（后者只在 `.escode` 空时参与）。
async fn discover_from_sources(
    descriptors: &[SourceDescriptor],
    root_for: impl Fn(&SourceDescriptor) -> PathBuf,
    scope: &'static str,
    commands: &mut Vec<Value>,
    overrides: &BTreeMap<String, bool>,
    project_path: Option<&str>,
) {
    for descriptor in descriptors {
        let before = commands.len();
        discover_root(
            descriptor,
            &root_for(descriptor),
            scope,
            commands,
            overrides,
            project_path,
        )
        .await;
        // `.escode` 读到有效命令后，同 scope 的 `.agents` 不再参与。
        if descriptor.directory_source == "escode" && commands.len() > before {
            break;
        }
    }
}

/// TS `discoverCommandsRoot`：根不存在即 0 条；扫描出错时保留已发现的部分。
async fn discover_root(
    descriptor: &SourceDescriptor,
    root: &Path,
    scope: &'static str,
    commands: &mut Vec<Value>,
    overrides: &BTreeMap<String, bool>,
    project_path: Option<&str>,
) {
    if !tokio::fs::try_exists(root).await.unwrap_or(false) {
        return;
    }
    discover_recursive(
        descriptor,
        root,
        root,
        scope,
        commands,
        overrides,
        project_path,
    )
    .await;
}

/// TS `discoverUserCommandsRecursive`：隐藏项跳过、目录递归、`.md` 文件解析成 `UserCommand`。
fn discover_recursive<'a>(
    descriptor: &'a SourceDescriptor,
    root: &'a Path,
    current: &'a Path,
    scope: &'static str,
    commands: &'a mut Vec<Value>,
    overrides: &'a BTreeMap<String, bool>,
    project_path: Option<&'a str>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let Ok(mut entries) = tokio::fs::read_dir(current).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let full = entry.path();
            let Ok(meta) = tokio::fs::symlink_metadata(&full).await else {
                continue;
            };
            if meta.is_dir() {
                discover_recursive(
                    descriptor,
                    root,
                    &full,
                    scope,
                    commands,
                    overrides,
                    project_path,
                )
                .await;
                continue;
            }
            if !name
                .to_ascii_lowercase()
                .ends_with(descriptor.file_extension)
            {
                continue;
            }
            let Ok(content) = tokio::fs::read_to_string(&full).await else {
                continue;
            };
            let full_text = full.to_string_lossy().into_owned();
            let Some(parsed) = parse_command_file(&content, &full_text) else {
                continue;
            };
            let name = command_name(root, &full, descriptor);
            let location = location(descriptor, root, scope, project_path);
            let mut command = parsed_fields(&parsed);
            command.insert("name".to_owned(), Value::from(name.clone()));
            command.insert(
                "agentSource".to_owned(),
                Value::from(descriptor.agent_source),
            );
            command.insert("location".to_owned(), location.clone());
            command.insert(
                "id".to_owned(),
                Value::from(format!(
                    "{}:{}:{}:{}",
                    descriptor.agent_source,
                    location["source"].as_str().unwrap_or_default(),
                    scope,
                    name
                )),
            );
            command.insert("source".to_owned(), Value::from("user"));
            command.insert(
                "enabled".to_owned(),
                Value::Bool(is_enabled(overrides, &full_text)),
            );
            command.insert("scope".to_owned(), Value::from(scope));
            if let Some(project) = project_path {
                command.insert("projectPath".to_owned(), Value::from(project));
            }
            commands.push(Value::Object(command));
        }
    })
}

/// TS `dedupeCommandsByName`：同名只保留**先出现**的那条。
fn dedupe_by_name(commands: Vec<Value>) -> Vec<Value> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut selected = Vec::new();
    for command in commands {
        let name = command["name"].as_str().unwrap_or_default().to_owned();
        if seen.insert(name) {
            selected.push(command);
        }
    }
    selected
}

async fn discover_plugin_commands(
    home: &Path,
    config: &Value,
    overrides: &BTreeMap<String, bool>,
    compare: &Compare,
) -> Vec<Value> {
    let descriptors = resolve_root_descriptors(home, config).await;
    let mut commands: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for descriptor in &descriptors {
        discover_plugin_recursive(
            descriptor,
            &descriptor.root_path,
            &mut commands,
            overrides,
            &mut seen,
        )
        .await;
    }
    commands.sort_by(|left, right| {
        compare(
            left["name"].as_str().unwrap_or_default(),
            right["name"].as_str().unwrap_or_default(),
        )
    });
    commands
}

/// TS `discoverPluginCommandsRecursive`：与用户命令同形，但多三件——`seen` 去重键是
/// **归一化小写文件路径**、`enabled` 还要求插件本身启用、输出是 `PluginCommand`。
fn discover_plugin_recursive<'a>(
    descriptor: &'a RootDescriptor,
    current: &'a Path,
    commands: &'a mut Vec<Value>,
    overrides: &'a BTreeMap<String, bool>,
    seen: &'a mut BTreeSet<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let Ok(mut entries) = tokio::fs::read_dir(current).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let full = entry.path();
            let Ok(meta) = tokio::fs::symlink_metadata(&full).await else {
                continue;
            };
            if meta.is_dir() {
                discover_plugin_recursive(descriptor, &full, commands, overrides, seen).await;
                continue;
            }
            let escode = descriptor_for(Some("escodeAgent")).expect("内置来源");
            if !name.to_ascii_lowercase().ends_with(escode.file_extension) {
                continue;
            }
            let Ok(content) = tokio::fs::read_to_string(&full).await else {
                continue;
            };
            let full_text = full.to_string_lossy().into_owned();
            let Some(parsed) = parse_command_file(&content, &full_text) else {
                continue;
            };
            let key = full_text.replace('\\', "/").to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let name = command_name(&descriptor.root_path, &full, escode);
            let mut command = parsed_fields(&parsed);
            command.insert(
                "enabled".to_owned(),
                Value::Bool(descriptor.plugin_enabled && is_enabled(overrides, &full_text)),
            );
            command.insert("filePath".to_owned(), Value::from(full_text.clone()));
            command.insert(
                "id".to_owned(),
                Value::from(format!(
                    "plugin:{}:{}:{}:{}",
                    descriptor.plugin_marketplace, descriptor.plugin_name, name, full_text
                )),
            );
            command.insert("name".to_owned(), Value::from(name));
            command.insert(
                "pluginEnabled".to_owned(),
                Value::Bool(descriptor.plugin_enabled),
            );
            command.insert(
                "pluginMarketplace".to_owned(),
                Value::from(descriptor.plugin_marketplace.clone()),
            );
            command.insert(
                "pluginName".to_owned(),
                Value::from(descriptor.plugin_name.clone()),
            );
            command.insert("scope".to_owned(), Value::from("global"));
            command.insert("source".to_owned(), Value::from("plugin"));
            commands.push(Value::Object(command));
        }
    })
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
