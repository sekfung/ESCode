//! user / workspace agent 的文件发现与内置摘要（TS `subagentsService.ts` 的
//! `createBuiltInAgents` / `discoverFileAgents` / `collectAgentMarkdownPaths` / 优先级三法）。
//!
//! 文件侧与插件侧有一处**刻意不同**：自定义 profile **只扫第一层**（插件才递归）——
//! 这是 TS 注释里点明的既有规则。

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::storage;
use super::summary::{SummaryScope, parse_subagent_markdown};

type Compare = dyn Fn(&str, &str) -> Ordering + Send + Sync;

/// TS `createBuiltInAgents`：两个内置 agent 是**常量**，只把模型覆盖回填进去。
pub fn built_in_agents(overrides: &Value) -> Vec<Value> {
    let general = overrides.get("general-purpose").cloned();
    let explore = overrides.get("Explore").cloned();
    vec![
        json!({
            "id": "built-in:built-in:general-purpose",
            "name": "general-purpose",
            "description": "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks.",
            "color": "blue",
            "injectAgentsMd": true,
            "modelSelection": general,
            "modelSelectionOverride": general,
            "systemPrompt": "",
            "tools": ["*"],
            "path": "built-in:general-purpose",
            "scope": "built-in",
            "source": "built-in",
            "enabled": true,
            "readOnly": true,
        }),
        json!({
            "id": "built-in:built-in:explore",
            "name": "Explore",
            "description": "Read-only search agent for broad fan-out searches.",
            "color": "cyan",
            "injectAgentsMd": false,
            "modelSelection": explore,
            "modelSelectionOverride": explore,
            "systemPrompt": "",
            "tools": ["Bash", "Glob", "Grep", "Read", "WebFetch", "WebSearch", "TodoWrite"],
            "path": "built-in:Explore",
            "scope": "built-in",
            "source": "built-in",
            "enabled": true,
            "readOnly": true,
        }),
    ]
}

/// TS `resolveCapabilities`：Host 恒为桌面运行时。
pub fn capability() -> Value {
    json!({"userScopeAvailable": true})
}

/// TS `collectAgentMarkdownPaths`：根必须是目录；只收 `.*md`/`*.markdown`（大小写不敏感）。
async fn collect_agent_markdown_paths(
    root: &Path,
    recursive: bool,
    compare: &Compare,
) -> Vec<String> {
    let Ok(meta) = tokio::fs::symlink_metadata(root).await else {
        return Vec::new();
    };
    if !meta.is_dir() {
        return Vec::new();
    }
    let mut paths: Vec<String> = Vec::new();
    collect(root, recursive, &mut paths).await;
    paths.sort_by(|left, right| compare(left, right));
    paths
}

fn collect<'a>(
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
                    collect(&path, true, paths).await;
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

/// TS `discoverFileAgents`：先用户根、再工作区根；读失败产出 `agent_read_failed`。
pub async fn discover_file_agents(
    home: &Path,
    workspace_path: &str,
    include_user_agents: bool,
    include_workspace_agents: bool,
    diagnostics: &mut Vec<Value>,
    compare: &Compare,
) -> Vec<Value> {
    let mut roots: Vec<(SummaryScope, std::path::PathBuf)> = Vec::new();
    if include_user_agents {
        roots.push((SummaryScope::User, storage::user_agents_root(home).await));
    }
    if include_workspace_agents {
        roots.push((
            SummaryScope::Workspace,
            storage::workspace_agents_root(workspace_path),
        ));
    }

    let mut agents = Vec::new();
    for (scope, root) in roots {
        for agent_path in collect_agent_markdown_paths(&root, false, compare).await {
            let Ok(markdown) = tokio::fs::read_to_string(&agent_path).await else {
                diagnostics.push(json!({
                    "code": "agent_read_failed",
                    "message": format!("Failed to read agent Markdown: {agent_path}"),
                    "path": agent_path,
                }));
                continue;
            };
            let outcome = parse_subagent_markdown(&markdown, &agent_path, scope);
            if let Some(diagnostic) = outcome.diagnostic {
                diagnostics.push(diagnostic);
                continue;
            }
            let Some(mut agent) = outcome.agent else {
                continue;
            };
            if scope == SummaryScope::Workspace {
                agent["projectPath"] = Value::from(workspace_path);
            }
            agents.push(agent);
        }
    }
    agents
}

/// TS `attachEnabledState`：**只有 user scope** 受禁用集影响（内置与插件恒为启用）。
pub fn attach_enabled_state(agents: Vec<Value>, state: &Value) -> Vec<Value> {
    let disabled: Vec<String> = state
        .get("disabledAgentIds")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    agents
        .into_iter()
        .map(|mut agent| {
            let scope = agent["scope"].as_str().unwrap_or_default();
            let id = agent["id"].as_str().unwrap_or_default();
            agent["enabled"] = Value::Bool(if scope == "user" {
                !disabled.iter().any(|candidate| candidate == id)
            } else {
                true
            });
            agent
        })
        .collect()
}

/// TS `applyRuntimePrecedence`：按 `name` 后者覆盖前者（Map 语义，保持首次出现的插入位置）。
pub fn apply_runtime_precedence(agents: Vec<Value>) -> Vec<Value> {
    let mut by_name: BTreeMap<String, Value> = BTreeMap::new();
    for agent in agents {
        by_name.insert(agent["name"].as_str().unwrap_or_default().to_owned(), agent);
    }
    by_name.into_values().collect()
}
