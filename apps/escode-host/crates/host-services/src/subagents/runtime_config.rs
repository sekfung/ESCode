//! `subagents/readRuntimeConfig` 的完整运行时快照（TS `createSubagentRuntimeConfigCache`）。
//!
//! Host 是解析结果的唯一 owner：这里只返回已解析 profile 与 state 覆盖，不把 Markdown
//! 交给 runtime 二次解释。读取失败按协议回退为 `built-in-fallback`，让 runtime 退回内置
//! 子代理；单个坏 Markdown 只跳过该文件，不污染整份快照。
//!
//! 与 TS 的 chokidar 缓存不同，Rust 在每次 RPC 时从磁盘生成快照。runtime 只在会话初始化
//! 与每个普通父轮开始时请求一次，因此线缆语义仍是“本轮固定一份快照”；差异只在内核实现
//! （无后台 watcher 与批次合并），不会让同一轮看到部分读取结果。

use std::cmp::Ordering;
use std::path::Path;

use escode_host_control::logging;
use serde_json::{Value, json};

use super::profile::{ProfileSource, parse_agent_profile_from_markdown};
use super::state;
use super::storage;
use super::summary::{SummaryScope, agent_state_id};

type Compare = dyn Fn(&str, &str) -> Ordering + Send + Sync;

/// TS `readRuntimeConfig`：成功返回 `{kind:"ready",...}`，任一读取失败返回显式回退。
pub async fn read_runtime_config(home: &Path, workspace_path: &str, compare: &Compare) -> Value {
    match ready_snapshot(home, workspace_path, compare).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            logging::warn(
                "subagents",
                format!("subagent runtime config read failed: {error}"),
            );
            json!({"kind": "built-in-fallback"})
        }
    }
}

async fn ready_snapshot(
    home: &Path,
    workspace_path: &str,
    compare: &Compare,
) -> Result<Value, String> {
    let state = state::read_state(&storage::state_file(home).await).await?;
    let mut profiles = Vec::new();
    // 与 TS 的拼接顺序一致：用户根在前，工作区根在后；每个根内部按完整路径排序。
    load_documents(
        &storage::user_agents_root(home).await,
        ProfileSource::User,
        &mut profiles,
        compare,
    )
    .await?;
    load_documents(
        &storage::workspace_agents_root(workspace_path),
        ProfileSource::Project,
        &mut profiles,
        compare,
    )
    .await?;

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
    // 启停只作用于用户创建的 profile；项目级与内置覆盖不走禁用集。
    profiles.retain(|profile| {
        if profile["source"].as_str() != Some("user") {
            return true;
        }
        let name = profile["name"].as_str().unwrap_or_default();
        let id = agent_state_id(name, SummaryScope::User, "user");
        !disabled.iter().any(|candidate| candidate == &id)
    });

    Ok(json!({
        "kind": "ready",
        "profiles": profiles,
        "builtInModelSelectionOverrides": table(&state, "builtInModelSelectionOverrides"),
        "pluginAgentModelSelectionOverrides": table(&state, "pluginAgentModelSelectionOverrides"),
    }))
}

fn table(state: &Value, key: &str) -> Value {
    state
        .get(key)
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

async fn load_documents(
    root: &Path,
    source: ProfileSource,
    profiles: &mut Vec<Value>,
    compare: &Compare,
) -> Result<(), String> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{error}")),
    };
    let mut paths = Vec::new();
    loop {
        let entry = entries
            .next_entry()
            .await
            .map_err(|error| format!("{error}"))?;
        let Some(entry) = entry else {
            break;
        };
        let file_type = entry
            .file_type()
            .await
            .map_err(|error| format!("{error}"))?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".md") || name.ends_with(".markdown") {
            paths.push(entry.path());
        }
    }
    paths.sort_by(|left, right| compare(&left.to_string_lossy(), &right.to_string_lossy()));

    for path in paths {
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            // 列目录与读取之间被删除：与 TS 按实际文件不存在处理一致。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{error}")),
        };
        let path_text = path.to_string_lossy().into_owned();
        match parse_agent_profile_from_markdown(&content, Some(&path_text), source) {
            Ok(outcome) => {
                if let Some(diagnostic) = outcome.diagnostic {
                    logging::warn(
                        "subagents",
                        format!(
                            "agent profile diagnostic: {} ({})",
                            diagnostic.message,
                            diagnostic.path.as_deref().unwrap_or(&path_text)
                        ),
                    );
                }
                profiles.push(outcome.profile);
            }
            Err(diagnostic) => logging::warn(
                "subagents",
                format!(
                    "agent profile skipped: {} ({})",
                    diagnostic.message,
                    diagnostic.path.as_deref().unwrap_or(&path_text)
                ),
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "runtime_config_tests.rs"]
mod tests;
