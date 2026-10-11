//! 会话持久化端口（`SessionStore`）与提交回执。

#[allow(unused_imports)]
use super::contract::*;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use escode_cli_domain::session::Session;

/// Receipt returned after the storage transaction is durable. The core uses
/// this boundary before advancing the model/tool loop or publishing facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableCommitReceipt {
    pub receipt_id: String,
    pub workspace: String,
    pub session_id: Option<String>,
    pub sequence: u64,
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Startup reads only the lightweight persisted index, never every transcript or ACK.
    async fn load_index(&self, workspace: &str) -> Result<BTreeMap<String, Value>>;
    /// Read one durable receipt, disposing a cold queued input without hydrating its session.
    async fn lookup_ack(&self, workspace: &str, key: &str) -> Result<Option<Value>>;
    /// Read persisted identities without loading history, resuming a runtime, or writing storage.
    async fn list_sessions(
        &self,
        _params: &escode_cli_domain::session_listing::ListParams,
        _owner: (&str, &str),
    ) -> Result<Vec<escode_cli_domain::session_listing::SessionListing>> {
        anyhow::bail!("Session listing unavailable")
    }
    /// Load one persisted conversation without activating a runtime or enumerating other history.
    async fn load_session(&self, _workspace: &str, _id: &str) -> Result<Option<Session>> {
        anyhow::bail!("Single session loading unavailable")
    }
    /// 项目权限规则：缺省表示该实现不持久化，「总是允许」选项因此不可用。
    /// ReadSessionContext：跨 workspace 读已提交历史（TS 形态，含 TS 库回落）。
    async fn session_context(
        &self,
        _id: &str,
    ) -> Result<Option<escode_cli_domain::session_context::SessionSource>> {
        Ok(None)
    }
    async fn load_project_rules(&self, _workspace: &str) -> Result<Option<Value>> {
        Ok(None)
    }
    /// 模型用量（TS usage store）：`{op: "record", fact}` 记录一次逻辑请求，`{op: "task", sessionId}` 按会话聚合。
    async fn usage(&self, _request: Value) -> Result<Value> {
        anyhow::bail!("Usage store is not available")
    }
    /// `workflows/runs` 的 journal 读面（docs/specs/rust-dynamic-workflow.md 第 4 期前置）。
    /// 缺省表示这个 store 没有 dwf journal——TS 在 journal 缺席时同样回空页而不是报错。
    async fn workflow_runs(
        &self,
        _query: &escode_cli_domain::dwf_journal::RunQuery,
    ) -> Result<Vec<escode_cli_domain::dwf_journal::JournalRun>> {
        Ok(Vec::new())
    }
    async fn save_project_rules(&self, _workspace: &str, _rules: &Value) -> Result<()> {
        anyhow::bail!("Project permission rules are not persisted by this store")
    }
    /// Reclaim only a draft with no history, atomically with the close receipt.
    async fn discard_draft(
        &self,
        _workspace: &str,
        _id: &str,
        _ack: (String, Value),
    ) -> Result<()> {
        anyhow::bail!("Draft reclamation unavailable")
    }
    async fn put_attachment(
        &self,
        _chunks: &[Vec<u8>],
        _mime: &str,
    ) -> Result<escode_cli_domain::session::StoredAttachment> {
        anyhow::bail!("Attachment storage unavailable")
    }
    async fn snapshot_attachment(
        &self,
        _path: &str,
        _mime: &str,
    ) -> Result<escode_cli_domain::session::StoredAttachment> {
        anyhow::bail!("Attachment snapshot unavailable")
    }
    async fn read_attachment(
        &self,
        _asset: &escode_cli_domain::session::StoredAttachment,
        _offset: u64,
        _limit: usize,
    ) -> Result<Vec<u8>> {
        anyhow::bail!("Attachment read unavailable")
    }
    async fn load(&self, workspace: &str) -> Result<(Vec<Session>, BTreeMap<String, Value>)>;
    async fn commit(
        &self,
        workspace: &str,
        session: Option<&mut Session>,
        ack: Option<(String, Value)>,
    ) -> Result<()>;
    async fn commit_receipt(
        &self,
        workspace: &str,
        session: Option<&mut Session>,
        ack: Option<(String, Value)>,
    ) -> Result<DurableCommitReceipt> {
        let session_id = session.as_ref().map(|value| value.id.clone());
        let sequence = session.as_ref().map(|value| value.seq).unwrap_or_default();
        self.commit(workspace, session, ack).await?;
        let receipt_id = format!(
            "{}:{}:{}",
            workspace,
            session_id.as_deref().unwrap_or("workspace"),
            sequence
        );
        Ok(DurableCommitReceipt {
            receipt_id,
            workspace: workspace.to_owned(),
            session_id,
            sequence,
        })
    }
}
