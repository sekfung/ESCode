use super::Engine;
use crate::{
    contract::StorageCommitFailure,
    domain::{claude_import, shared_import::Import},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

impl Engine {
    /// `session/create` + `importedHistory.source="claudeCode"`（docs/specs/rust-legacy-session-methods.md）：
    /// 一次提交建出可续聊的会话；同 ID 已存在时返回现有快照，不重写历史。
    pub(super) async fn import_claude_history(&mut self, value: &Value) -> Result<Value> {
        let mut input = Import::parse_claude(value)?;
        ensure!(
            input.workspace.identity() == self.workspace
                && input.workspace.workspace_path == self.workspace_path,
            "Workspace identity mismatch"
        );
        let id = input.session_id.take().unwrap_or_else(|| self.clock.id());
        if !self.sessions.contains_key(&id)
            && self
                .store
                .load_session(&self.workspace, &id)
                .await?
                .is_some()
        {
            self.ensure_session(&id).await?;
        }
        if self.sessions.contains_key(&id) {
            return self.read_session(&json!({"sessionId":id}));
        }
        let history = &input.imported_history;
        let (created, updated) = claude_import::times(history, self.clock.now());
        let mut session = self.imported_session(&id, &input, created)?;
        session.title = history
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("Imported session")
            .into();
        claude_import::build(&mut session, history, created);
        session.updated_at = session.updated_at.max(updated);
        if let Some(servers) = &input.mcp_servers {
            self.tools.configure_mcp(&id, &json!(servers)).await?;
        }
        self.store
            .commit_receipt(&self.workspace, Some(&mut session), None)
            .await
            .context(StorageCommitFailure)?;
        self.sessions.insert(id.clone(), session);
        self.publish(&id, vec![])?;
        self.read_session(&json!({"sessionId":id}))
    }
}
