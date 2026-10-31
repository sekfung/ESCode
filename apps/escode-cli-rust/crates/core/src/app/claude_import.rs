use super::Engine;
use crate::{
    contract::{ModelIdentity, StorageCommitFailure},
    domain::{claude_import, claude_import::Import, claude_reimport, session::Session},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

impl Engine {
    /// 导入会话的公共骨架：模型选择、workspace、mode、custom 标题来源。
    fn imported_session(&self, id: &str, input: &Import, created_at: u64) -> Result<Session> {
        let config = if input.model.is_some() {
            self.select(
                &json!({"modelSelection":input.model,"thought":input.thought_level}),
                None,
            )?
        } else {
            self.config.clone().unwrap_or(ModelIdentity {
                provider_id: String::new(),
                model_id: String::new(),
                reasoning_level: String::new(),
            })
        };
        let mut session = Session::new(
            id.into(),
            self.workspace.clone(),
            config.provider_id,
            config.model_id,
            config.reasoning_level,
            self.clock.id(),
            created_at,
        );
        session.workspace_path = Some(self.workspace_path.clone());
        session.workspace_directory = Some(self.workspace_path.clone());
        session.parent_id = input.parent_session_id.clone();
        session.trace_id = Some(self.clock.id());
        session.title_source = "custom".into();
        session.phase = "completedSuccess".into();
        if let Some(mode) = input.mode.clone() {
            session.plan_enabled = mode == "plan";
            session.mode = if mode == "plan" || mode == "auto" {
                "build".into()
            } else {
                mode
            };
        }
        Ok(session)
    }

    /// `session/create` + `importedHistory.source="claudeCode"`（docs/specs/rust-legacy-session-methods.md）：
    /// 一次提交建出可续聊的会话；同 ID 已存在时按「重导入与历史替换」重写导入段并保留续聊。
    pub(super) async fn import_claude_history(&mut self, value: &Value) -> Result<Value> {
        let mut input = Import::parse(value)?;
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
            let (created, updated) =
                claude_import::times(&input.imported_history, self.clock.now());
            return self
                .reimport_claude_history(&id, &input, created, updated)
                .await;
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

    /// 同 ID 重导入：对齐 TS `persistImportedSessionHistory` —— 删除旧导入段、按新历史回填并保留续聊。
    /// 重写是全量的：提交后换 epoch、seq 归零并给订阅者发 recovery 快照，客户端整体替换行。
    async fn reimport_claude_history(
        &mut self,
        id: &str,
        input: &Import,
        created: u64,
        updated: u64,
    ) -> Result<Value> {
        let s = &self.sessions[id];
        ensure!(
            !s.running()
                && s.pending.is_empty()
                && s.queue.is_empty()
                && !s.background.values().any(|task| task.status == "running"),
            "Cannot re-import history while the session is active"
        );
        let config = if input.model.is_some() {
            Some(self.select(
                &json!({"modelSelection":input.model,"thought":input.thought_level}),
                Some(self.session_selection(id)?),
            )?)
        } else {
            None
        };
        // Node 重建 runtime record 时按本次参数重配 MCP；未传即空表（修复调用不带 mcpServers）。
        let servers = input.mcp_servers.clone().unwrap_or_default();
        self.tools.configure_mcp(id, &json!(servers)).await?;
        let mode = input.mode.as_deref().unwrap_or("build").to_owned();
        let s = self.sessions.get_mut(id).unwrap();
        claude_reimport::replace(s, &input.imported_history, created);
        if let Some(config) = config {
            s.provider = config.provider_id;
            s.model = config.model_id;
            s.reasoning_level = config.reasoning_level;
        }
        s.title = input
            .imported_history
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Imported session")
            .into();
        s.title_source = "custom".into();
        s.parent_id = input.parent_session_id.clone();
        s.task_type = "interactive".into();
        s.plan_enabled = mode == "plan";
        s.mode = if matches!(mode.as_str(), "plan" | "auto") {
            "build"
        } else {
            mode.as_str()
        }
        .into();
        // Node `createSession` 的 upsert 只覆盖 time_updated，createdAt 保留。
        s.updated_at = updated;
        s.last_error = None;
        s.api_retry = None;
        s.run_id = None;
        let _ = s.history_actions();
        s.epoch = self.clock.id();
        s.seq = 0;
        s.revision += 1;
        self.persist(id, None).await?;
        self.history_snapshot(id)?;
        self.read_session(&json!({"sessionId":id}))
    }
}
