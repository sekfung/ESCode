//! `subagents` 域（docs/specs/rust-u2-subagents.md）。
//!
//! **本片交付 4/8**：目录解析 + 状态文件写入面（`disabledAgentIds` 与两张模型覆盖表）。
//! 其余 4 个（`list` / `createAgent` / `updateAgent` / `deleteAgent`）依赖 markdown 解析与生成
//! （`subagentMarkdown.ts` 435 行）、多来源发现（用户 / 工作区 / 插件清单）与 chokidar 式的
//! 热重载作用域（`subagentConfigScope.ts` 343 行），是同一域里明显更大的另一半切片。
//!
//! **所有写操作串行**（TS `enqueue` 的写队列）：读-改-写必须是一个整体，否则并发 setEnabled 会丢更新。

pub mod discovery_file;
pub mod discovery_plugin;
pub mod discovery_plugin_records;
pub mod frontmatter;
pub mod frontmatter_value;
pub mod list;
pub mod model_selection;
pub mod model_value;
pub mod profile;
pub mod serialize;
pub mod state;
pub mod storage;
pub mod summary;
pub mod write;
pub mod yaml_json;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

/// Host 内唯一的 subagents 服务（`home` = 设置域根）。
pub struct SubagentsService {
    home: PathBuf,
    /// 写队列：`setEnabled` / 两个模型覆盖的读-改-写串行化。
    queue: Arc<Mutex<()>>,
}

impl SubagentsService {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            queue: Arc::new(Mutex::new(())),
        }
    }

    /// 写队列（写三法与状态写入面共用同一把锁，保证读-改-写串行）。
    pub(super) fn queue(&self) -> &Arc<Mutex<()>> {
        &self.queue
    }

    /// 设置域根（`list` 的发现层需要它定位用户根与状态文件）。
    pub fn home(&self) -> &std::path::Path {
        &self.home
    }

    /// TS `getPrimaryUserAgentsDirectory`：解析用户 agent 根并**确保它落盘**，回原样路径。
    pub async fn primary_user_agents_directory(&self) -> Result<String, String> {
        let path = storage::user_agents_root(&self.home).await;
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|error| format!("{error}"))?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// TS `setEnabled`：`disabledAgentIds` 去重后**升序**写回。
    pub async fn set_enabled(&self, agent_id: &str, enabled: bool) -> Result<(), String> {
        let _guard = self.queue.lock().await;
        let path = storage::state_file(&self.home).await;
        let mut current = state::read_state(&path).await?;
        let mut ids: Vec<String> = current
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
        ids.retain(|id| id != agent_id);
        if !enabled {
            ids.push(agent_id.to_owned());
        }
        ids.sort();
        current["disabledAgentIds"] = Value::Array(ids.into_iter().map(Value::from).collect());
        state::write_state(&path, &current).await
    }

    /// TS `setBuiltInModelOverride`：只改 `builtInModelSelectionOverrides`；
    /// `modelSelection` 缺省/`null` 表示**清除**该覆盖。
    pub async fn set_built_in_model_override(
        &self,
        agent_name: &str,
        selection: Option<&Value>,
    ) -> Result<(), String> {
        let selection = model_selection::normalize(selection)?;
        let _guard = self.queue.lock().await;
        let path = storage::state_file(&self.home).await;
        let mut current = state::read_state(&path).await?;
        apply_override(
            &mut current,
            "builtInModelSelectionOverrides",
            agent_name,
            selection,
        );
        state::write_state(&path, &current).await
    }

    /// TS `setPluginAgentModelOverride`：id 必须是 `plugin:` 前缀**且无首尾空白**。
    pub async fn set_plugin_agent_model_override(
        &self,
        agent_id: &str,
        selection: Option<&Value>,
    ) -> Result<(), String> {
        if !agent_id.starts_with("plugin:") || agent_id.trim() != agent_id {
            return Err("无效插件 Subagent 身份".to_owned());
        }
        let selection = model_selection::normalize(selection)?;
        let _guard = self.queue.lock().await;
        let path = storage::state_file(&self.home).await;
        let mut current = state::read_state(&path).await?;
        apply_override(
            &mut current,
            "pluginAgentModelSelectionOverrides",
            agent_id,
            selection,
        );
        state::write_state(&path, &current).await
    }
}

/// 覆盖表增删：有值则写入，无值则删键（不清空整张表，也不动其它条目）。
fn apply_override(current: &mut Value, table: &str, key: &str, selection: Option<Value>) {
    let mut entries = current
        .get(table)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    match selection {
        Some(selection) => {
            entries.insert(key.to_owned(), selection);
        }
        None => {
            entries.remove(key);
        }
    }
    current[table] = Value::Object(entries);
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
