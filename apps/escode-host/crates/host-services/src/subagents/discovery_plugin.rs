//! 插件 agent 的来源发现（TS `subagentsService.ts` 的 `discoverPluginAgents` 一族）。
//!
//! 与 `commands` 的插件扫描**不是同一套**：这里按插件名分组（`{name, versionRoots}`），
//! 版本目录做「数字感知、忽略大小写、降序」排序并过滤瞬态目录；只有第一个存在的版本根参与。
//!
//! 三条容易漏的规则：
//! 1. **卸载抑制优先于遗留安装记录**：`installed_plugins.json` 里被 `suppressedBuiltins` 抑制的记录不能用；
//! 2. 内置插件没有安装记录，靠缓存兜底，但缓存**不能复活**被抑制的插件；
//! 3. 插件 agent 的**裸名**唯一且不与已有名字冲突时，额外产出一条运行时别名（UI 投影只用带插件名前缀的）。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use super::discovery_plugin_records::{read_enabled_records, read_manifest, read_plugin_config};
use super::storage;
use super::summary::{SummaryScope, parse_subagent_markdown, plugin_agent_state_id};

type Compare = dyn Fn(&str, &str) -> Ordering + Send + Sync;

#[derive(Clone)]
pub struct PluginRecord {
    pub id: String,
    pub install_path: PathBuf,
    pub name: String,
}

pub struct PluginDiscovery {
    pub profiles: Vec<Value>,
    pub runtime_agents: Vec<Value>,
    pub diagnostics: Vec<Value>,
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// TS `collectPluginAgentRoots`：`agents` + 清单 `agents` 字段（字符串或字符串数组），去 `./`、
/// 且归一化后必须仍在插件根内。
fn collect_agent_roots(root: &Path, manifest_agents: Option<&Value>) -> Vec<PathBuf> {
    let root = normalize_lexical(root);
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut add = |relative: &str| {
        let cleaned = relative.strip_prefix("./").unwrap_or(relative);
        let resolved = normalize_lexical(&root.join(cleaned));
        if resolved.strip_prefix(&root).is_err() {
            return;
        }
        if !roots.iter().any(|existing| existing == &resolved) {
            roots.push(resolved);
        }
    };
    add("agents");
    match manifest_agents {
        Some(Value::String(text)) => add(text),
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    add(text);
                }
            }
        }
        _ => {}
    }
    roots
}

/// TS `collectPluginAgentMarkdownPaths`：插件侧**递归**收 `.*md`。
async fn collect_plugin_agent_markdown_paths(root: &Path, compare: &Compare) -> Vec<String> {
    let manifest = read_manifest(root).await;
    let mut paths: Vec<String> = Vec::new();
    for agent_root in collect_agent_roots(root, manifest.as_ref().and_then(|m| m.get("agents"))) {
        collect_markdown(&agent_root, true, &mut paths).await;
    }
    paths.sort_by(|left, right| compare(left, right));
    paths
}

fn collect_markdown<'a>(
    directory: &'a Path,
    recursive: bool,
    paths: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                if recursive {
                    collect_markdown(&path, true, paths).await;
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".md") || name.ends_with(".markdown") {
                paths.push(path.to_string_lossy().into_owned());
            }
        }
    })
}

/// TS `applyPluginAgentOverrides`：插件文件只给默认值；用户覆盖**替换整份**选择。
fn apply_overrides(mut agent: Value, state: &Value) -> Value {
    let id = agent["id"].as_str().unwrap_or_default().to_owned();
    let override_selection = state
        .get("pluginAgentModelSelectionOverrides")
        .and_then(|table| table.get(&id))
        .cloned();
    let default = agent.get("modelSelection").cloned();
    if let Some(default) = default {
        agent["defaultModelSelection"] = default;
    }
    if let Some(selection) = override_selection {
        agent["modelSelection"] = selection.clone();
        agent["modelSelectionOverride"] = selection;
    }
    agent
}

pub async fn discover_plugin_agents(
    home: &Path,
    state: &Value,
    reserved_names: &[String],
    compare: &Compare,
) -> PluginDiscovery {
    let storage_root = storage::storage_root(home).await;
    let cli_storage_root = if storage_root.file_name().and_then(|name| name.to_str()) == Some("cli")
    {
        storage_root
    } else {
        storage_root.join("cli")
    };
    let config = read_plugin_config(home).await;
    let records = read_enabled_records(&cli_storage_root.join("plugins"), &config).await;

    struct Parsed {
        agent: Value,
        bare_name: String,
        plugin_name: String,
    }
    let mut parsed_agents: Vec<Parsed> = Vec::new();
    let mut diagnostics: Vec<Value> = Vec::new();
    for record in records {
        for agent_path in collect_plugin_agent_markdown_paths(&record.install_path, compare).await {
            let Ok(markdown) = tokio::fs::read_to_string(&agent_path).await else {
                diagnostics.push(json!({
                    "code": "agent_read_failed",
                    "message": format!("Failed to read plugin agent Markdown: {agent_path}"),
                    "path": agent_path,
                }));
                continue;
            };
            let outcome = parse_subagent_markdown(&markdown, &agent_path, SummaryScope::User);
            if let Some(diagnostic) = outcome.diagnostic {
                diagnostics.push(diagnostic);
                continue;
            }
            let Some(mut agent) = outcome.agent else {
                continue;
            };
            let bare_name = agent["name"].as_str().unwrap_or_default().to_owned();
            let id = plugin_agent_state_id(&record.id, &bare_name);
            agent["id"] = Value::from(id);
            agent["name"] = Value::from(format!("{}:{bare_name}", record.name));
            agent["readOnly"] = Value::Bool(true);
            agent["source"] = Value::from("plugin");
            agent["pluginId"] = Value::from(record.id.clone());
            agent["pluginName"] = Value::from(record.name.clone());
            parsed_agents.push(Parsed {
                agent: apply_overrides(agent, state),
                bare_name,
                plugin_name: record.name.clone(),
            });
        }
    }

    let mut reserved: BTreeSet<String> = ["general-purpose", "Explore"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for name in reserved_names {
        reserved.insert(name.clone());
    }
    let mut bare_counts: BTreeMap<String, usize> = BTreeMap::new();
    for parsed in &parsed_agents {
        *bare_counts.entry(parsed.bare_name.clone()).or_insert(0) += 1;
    }

    let mut profiles: Vec<Value> = Vec::new();
    let mut runtime_agents: Vec<Value> = Vec::new();
    for parsed in parsed_agents {
        profiles.push(parsed.agent.clone());
        runtime_agents.push(parsed.agent.clone());
        let count = bare_counts.get(&parsed.bare_name).copied().unwrap_or(0);
        if count == 1 && !reserved.contains(&parsed.bare_name) {
            let mut alias = parsed.agent.clone();
            alias["id"] = Value::from(plugin_agent_state_id(
                &format!("{}:alias", parsed.agent["id"].as_str().unwrap_or_default()),
                &parsed.bare_name,
            ));
            alias["name"] = Value::from(parsed.bare_name.clone());
            runtime_agents.push(alias);
        } else {
            diagnostics.push(json!({
                "code": "agent_ambiguous_name",
                "message": format!(
                    "Plugin agent bare name {}; use {}:{}",
                    if reserved.contains(&parsed.bare_name) {
                        "conflicts with an existing profile"
                    } else {
                        "is ambiguous"
                    },
                    parsed.plugin_name,
                    parsed.bare_name
                ),
                "path": parsed.agent["path"],
            }));
        }
    }

    let name_of = |agent: &Value| agent["name"].as_str().unwrap_or_default().to_owned();
    profiles.sort_by(|left, right| compare(&name_of(left), &name_of(right)));
    runtime_agents.sort_by(|left, right| compare(&name_of(left), &name_of(right)));
    PluginDiscovery {
        profiles,
        runtime_agents,
        diagnostics,
    }
}
