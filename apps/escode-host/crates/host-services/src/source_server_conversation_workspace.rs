//! `ensureConversationWorkspace` 的持久化半边（spec §9.8 / §10.4）：设备级 canonical cwd
//! 只写 `remoteConversationWorkspaceBySourceServerId`，并清掉同 Source 同路径的 project history。

use anyhow::{Result, bail};
use serde_json::json;

use crate::settings;

use super::super::{SourceServerRegistry, WorkspaceHistoryEntry};
use super::now_ms;

impl SourceServerRegistry {
    /// 记录远端会话 canonical workspace；同一 Source 的同路径 project history 视为污染项删除。
    pub async fn record_conversation_workspace(
        &self,
        source_server_id: &str,
        workspace_path: &str,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let workspace_path = workspace_path.trim();
        if source_server_id.trim().is_empty() || workspace_path.is_empty() {
            bail!("Source conversation workspace requires sourceServerId and workspacePath");
        }
        let home = self.settings_home()?;
        let current_settings = settings::read(home).await?;
        let mut map = current_settings
            .get("remoteConversationWorkspaceBySourceServerId")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        map.insert(
            source_server_id.to_owned(),
            json!({ "workspacePath": workspace_path, "lastResolvedAt": now_ms() }),
        );
        let history: Vec<WorkspaceHistoryEntry> = {
            let state = self.lock();
            state
                .workspace_history
                .iter()
                .filter(|entry| {
                    !(entry.source_server_id.as_deref() == Some(source_server_id)
                        && entry.workspace_path == workspace_path)
                })
                .cloned()
                .collect()
        };
        self.persist(json!({
            "remoteConversationWorkspaceBySourceServerId": map,
            "sourceServerWorkspaceHistory": history,
        }))
        .await?;
        self.apply_state(|state| state.workspace_history = history);
        Ok(())
    }
}
