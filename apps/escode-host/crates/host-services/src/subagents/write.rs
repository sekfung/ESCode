//! `subagents` 写三法：`createAgent` / `updateAgent` / `deleteAgent`（TS `subagentsService.ts:740-800`）。
//!
//! 三条语义要点：
//! 1. **写前先回解析**：序列化结果必须能被 `parseSubagentMarkdown` 重新读出，否则报错不落盘
//!    （防止 serializer 回归把坏 profile 写进用户目录）；
//! 2. **create 用 `wx`**（原子创建，文件已存在即失败），update 是覆盖写；
//! 3. **改名必须迁移禁用态**：`disabledAgentIds` 按 id 存，而 id 含 name——不迁移会让用户刚禁用的
//!    agent 因改名重新启用。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::SubagentsService;
use super::serialize::serialize_subagent_markdown;
use super::state;
use super::storage;
use super::summary::{SummaryScope, parse_subagent_markdown};

const BUILT_IN_NAMES: [&str; 2] = ["general-purpose", "Explore"];

/// TS `validateUserAgentConfig`。
fn validate_config(config: &Map<String, Value>) -> Result<(), String> {
    let name = config
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let length = name.chars().count();
    if !(3..=50).contains(&length) {
        return Err("Name must be between 3 and 50 characters".to_owned());
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("Name can only contain letters, numbers, and hyphens".to_owned());
    }
    let description = config
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if description.trim().is_empty() {
        return Err("Description is required".to_owned());
    }
    let prompt = config
        .get("systemPrompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if prompt.trim().is_empty() {
        return Err("System prompt is required".to_owned());
    }
    Ok(())
}

/// TS `assertNotBuiltInName`。
fn assert_not_built_in(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if BUILT_IN_NAMES.contains(&trimmed) {
        return Err(format!(
            "Agent name \"{trimmed}\" is reserved by a built-in agent"
        ));
    }
    Ok(())
}

/// TS `normalizeConfig`：三个文本字段 trim，`modelSelection` 走同一套校验/归一。
fn normalize_config(config: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut normalized = config.clone();
    for key in ["name", "description", "systemPrompt"] {
        let text = config.get(key).and_then(Value::as_str).unwrap_or_default();
        normalized.insert(key.to_owned(), Value::from(text.trim()));
    }
    if let Some(selection) = config
        .get("modelSelection")
        .filter(|value| !value.is_null())
    {
        match super::model_selection::normalize(Some(selection))? {
            Some(selection) => {
                normalized.insert("modelSelection".to_owned(), selection);
            }
            None => {
                normalized.remove("modelSelection");
            }
        }
    } else {
        normalized.remove("modelSelection");
    }
    Ok(normalized)
}

fn missing_file_name(agent: &str) -> String {
    format!("Missing agent file path: {agent}")
}

impl SubagentsService {
    /// 解析目标目录（用户根是异步的，单独一步）。
    async fn target_for(&self, params: &Value) -> Result<(PathBuf, &'static str), String> {
        let scope = params["scope"].as_str().unwrap_or("user");
        if scope == "workspace" {
            let workspace = params["workspacePath"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Workspace path is required for workspace subagents".to_owned())?;
            Ok((storage::workspace_agents_root(workspace), "workspace"))
        } else {
            Ok((storage::user_agents_root(self.home()).await, "user"))
        }
    }

    /// TS `createAgent`。
    pub async fn create_agent(&self, params: &Value) -> Result<Value, String> {
        let config = params["config"]
            .as_object()
            .ok_or_else(|| "config is required".to_owned())?;
        validate_config(config)?;
        assert_not_built_in(
            config
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?;
        let _guard = self.queue().lock().await;

        let (directory, scope) = self.target_for(params).await?;
        let normalized = normalize_config(config)?;
        let name = normalized["name"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        let file_path = directory.join(format!("{name}.md"));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("{error}"))?;
        if tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
            return Err(format!(
                "Agent file \"{}\" already exists",
                file_name(&file_path)
            ));
        }

        let content = serialize_subagent_markdown(&normalized);
        let agent = parse_saved_agent(&content, &file_path, scope, params)?;
        // `wx`：原子创建，避免与并发写入互相覆盖。
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut handle = options
            .open(&file_path)
            .await
            .map_err(|error| format!("{error}"))?;
        use tokio::io::AsyncWriteExt;
        handle
            .write_all(content.as_bytes())
            .await
            .map_err(|error| format!("{error}"))?;
        Ok(json!({ "agent": agent }))
    }

    /// TS `updateAgent`。
    pub async fn update_agent(&self, params: &Value) -> Result<Value, String> {
        let config = params["config"]
            .as_object()
            .ok_or_else(|| "config is required".to_owned())?;
        validate_config(config)?;
        assert_not_built_in(
            config
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?;
        let agent_id = params["agentId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "agentId is required".to_owned())?
            .to_owned();
        let _guard = self.queue().lock().await;

        let (directory, scope) = self.target_for(params).await?;
        let normalized = normalize_config(config)?;
        let name = normalized["name"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        let file_path = directory.join(format!("{name}.md"));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("{error}"))?;
        let old_path = params["oldFilePath"]
            .as_str()
            .map(str::to_owned)
            .filter(|value| !value.is_empty());
        let renamed = old_path
            .as_deref()
            .is_some_and(|old| Path::new(old) != file_path);
        if renamed && tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
            return Err(format!(
                "Agent file \"{}\" already exists",
                file_name(&file_path)
            ));
        }

        let content = serialize_subagent_markdown(&normalized);
        let agent = parse_saved_agent(&content, &file_path, scope, params)?;
        tokio::fs::write(&file_path, &content)
            .await
            .map_err(|error| format!("{error}"))?;
        // 改名会生成新 id：禁用集必须跟着迁移，否则刚禁用的 agent 会重新启用。
        migrate_disabled_agent_id(
            self.home(),
            &agent_id,
            agent["id"].as_str().unwrap_or_default(),
        )
        .await?;
        if let Some(old) = old_path.filter(|_| renamed) {
            let _ = tokio::fs::remove_file(old).await;
        }
        Ok(json!({ "agent": agent }))
    }

    /// TS `deleteAgent`：删文件并把该 id 从禁用集里移除（文件都没了，留着是悬空键）。
    pub async fn delete_agent(&self, params: &Value) -> Result<(), String> {
        let file_path = params["filePath"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| missing_file_name("<unknown>"))?;
        let agent_id = params["agentId"].as_str().unwrap_or_default().to_owned();
        let _guard = self.queue().lock().await;
        match tokio::fs::remove_file(file_path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{error}")),
            Ok(()) => {}
        }
        let state_path = storage::state_file(self.home()).await;
        let mut current = state::read_state(&state_path).await?;
        let mut ids: Vec<String> = current
            .get("disabledAgentIds")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|id| *id != agent_id)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        current["disabledAgentIds"] = Value::Array(ids.into_iter().map(Value::from).collect());
        state::write_state(&state_path, &current).await
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// TS `parseSavedAgent`：序列化结果必须能被重新解析成摘要，否则不落盘。
fn parse_saved_agent(
    content: &str,
    path: &Path,
    scope: &'static str,
    params: &Value,
) -> Result<Value, String> {
    let text = path.to_string_lossy().into_owned();
    let summary_scope = if scope == "workspace" {
        SummaryScope::Workspace
    } else {
        SummaryScope::User
    };
    let outcome = parse_subagent_markdown(content, &text, summary_scope);
    let Some(mut agent) = outcome.agent else {
        let message = outcome
            .diagnostic
            .as_ref()
            .and_then(|diagnostic| diagnostic["message"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Failed to parse saved agent: {text}"));
        return Err(message);
    };
    if scope == "workspace" {
        agent["projectPath"] = Value::from(
            params["workspacePath"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
    }
    Ok(agent)
}

/// TS `migrateDisabledAgentId`：旧 id 被禁用时，把禁用态搬到新 id。
async fn migrate_disabled_agent_id(
    home: &Path,
    previous_id: &str,
    next_id: &str,
) -> Result<(), String> {
    if previous_id == next_id {
        return Ok(());
    }
    let state_path = storage::state_file(home).await;
    let mut current = state::read_state(&state_path).await?;
    let ids: Vec<String> = current
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
    if !ids.iter().any(|id| id == previous_id) {
        return Ok(());
    }
    let mut next: Vec<String> = ids.into_iter().filter(|id| id != previous_id).collect();
    next.push(next_id.to_owned());
    next.sort();
    current["disabledAgentIds"] = Value::Array(next.into_iter().map(Value::from).collect());
    state::write_state(&state_path, &current).await
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
