//! Public runtime ports. The application owns state; adapters own external IO.
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
pub use zcode_cli_domain::model::{ModelFailure, RetryState};
use zcode_cli_domain::session::Session;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub provider_id: String,
    pub model_id: String,
    pub reasoning_level: String,
}
pub type Output = mpsc::Sender<Vec<Value>>;
pub struct ChildHandle {
    pub task: zcode_cli_domain::subagent::Task,
    pub updates: tokio::sync::watch::Receiver<zcode_cli_domain::subagent::Task>,
    pub message_id: Option<String>,
    pub delivery: Option<String>,
}
pub use crate::failures::{ProcessCleanupFailure, StorageCommitFailure};

/// Receipt returned after the storage transaction is durable. The core uses
/// this boundary before advancing the model/tool loop or publishing facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableCommitReceipt {
    pub receipt_id: String,
    pub workspace: String,
    pub session_id: Option<String>,
    pub sequence: u64,
}

pub use crate::contract_events::{
    Event, EventSink, HostReply, Input, ModelOutput, PermissionOutcome, RunEvent,
};
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Startup reads only the lightweight persisted index, never every transcript or ACK.
    async fn load_index(&self, workspace: &str) -> Result<BTreeMap<String, Value>>;
    /// Read one durable receipt, disposing a cold queued input without hydrating its session.
    async fn lookup_ack(&self, workspace: &str, key: &str) -> Result<Option<Value>>;
    /// Read persisted identities without loading history, resuming a runtime, or writing storage.
    async fn list_sessions(
        &self,
        _params: &zcode_cli_domain::session_listing::ListParams,
        _owner: (&str, &str),
    ) -> Result<Vec<zcode_cli_domain::session_listing::SessionListing>> {
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
    ) -> Result<Option<zcode_cli_domain::session_context::SessionSource>> {
        Ok(None)
    }
    async fn load_project_rules(&self, _workspace: &str) -> Result<Option<Value>> {
        Ok(None)
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
    ) -> Result<zcode_cli_domain::session::StoredAttachment> {
        anyhow::bail!("Attachment storage unavailable")
    }
    async fn snapshot_attachment(
        &self,
        _path: &str,
        _mime: &str,
    ) -> Result<zcode_cli_domain::session::StoredAttachment> {
        anyhow::bail!("Attachment snapshot unavailable")
    }
    async fn read_attachment(
        &self,
        _asset: &zcode_cli_domain::session::StoredAttachment,
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
#[async_trait]
pub trait ModelPort: Send + Sync {
    fn identity(&self) -> Option<ModelIdentity> {
        None
    }
    fn format_properties(&self) -> Value {
        serde_json::json!({"inputFormat":{"supportsText":true,"supportsImage":false,"supportsVideo":false,"supportsAudio":false,"supportsPdf":false},"outputFormat":{"supportsText":true}})
    }
    /// 模型是否声明 provider-native 搜索（TS `properties.supportsNativeWebSearch`）。
    fn native_web_search(&self) -> bool {
        false
    }
    fn with_max_output_tokens(&self, _max: usize) -> Result<Option<Arc<dyn ModelPort>>> {
        Ok(None)
    }
    fn bind(&self) -> Option<Arc<dyn ModelPort>> {
        None
    }
    /// 辅助调用用的最低推理档位绑定（TS `auxiliaryModelOptions`）；无注册表时为 None，沿用当前模型。
    fn auxiliary(&self) -> Option<Arc<dyn ModelPort>> {
        None
    }
    fn context_policy(&self) -> zcode_cli_domain::context::ContextPolicy {
        Default::default()
    }
    async fn complete(
        &self,
        messages: Vec<Value>,
        tools: &[Value],
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> std::result::Result<ModelOutput, ModelFailure>;
}
/// Runtime configuration is resolved outside the actor; credentials are never session facts.
#[async_trait]
pub trait ModelRegistry: Send + Sync {
    async fn received_account(&self) -> Option<Value> {
        None
    }
    fn catalog(&self) -> Vec<Value>;
    fn model_options(&self) -> Vec<Value> {
        self.catalog()
    }
    fn default_selection(&self) -> Option<ModelIdentity>;
    fn resolve(&self, selection: &ModelIdentity) -> Result<Arc<dyn ModelPort>>;
    async fn refresh(&self, account: Option<Value>) -> Result<bool>;
}
#[async_trait]
pub trait RewindTransaction: Send {
    fn preview(&self) -> Value;
    fn checkpoint_ids(&self) -> Vec<String>;
    async fn finish(self: Box<Self>, commit: bool) -> Result<()>;
}
#[async_trait]
pub trait ToolPort: Send + Sync {
    async fn file_changes(
        &self,
        _changes: &[zcode_cli_domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Value> {
        anyhow::bail!("File changes unavailable")
    }
    async fn pending_rewinds(&self) -> Result<Vec<String>> {
        Ok(vec![])
    }
    async fn recover_rewind(&self, _session: &str, _committed: Option<&str>) -> Result<()> {
        Ok(())
    }
    async fn rewind_preview(
        &self,
        _changes: &[zcode_cli_domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Value> {
        anyhow::bail!("File rewind unavailable")
    }
    async fn begin_rewind(
        &self,
        _session: &str,
        _token: &str,
        _changes: &[zcode_cli_domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Box<dyn RewindTransaction>> {
        anyhow::bail!("File rewind unavailable")
    }
    async fn agent_memory(
        &self,
        _profile: &zcode_cli_domain::subagent::Profile,
        _cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        Ok(None)
    }
    async fn agent_profiles(
        &self,
        _cancel: &CancellationToken,
    ) -> Result<Vec<zcode_cli_domain::subagent::Profile>> {
        Ok(zcode_cli_domain::subagent::builtins())
    }
    async fn inherit_session(&self, _parent: &str, _child: &str) -> Result<()> {
        Ok(())
    }
    /// 批准的计划写入 `<workspace>/.zcode/plans/<file_name>`；无文件系统时与 TS 一样跳过。
    async fn write_plan_file(&self, _file_name: &str, _plan: &str) -> Result<()> {
        Ok(())
    }
    async fn agent_output(&self, _session: &str, _text: &str) -> Result<String> {
        anyhow::bail!("Agent artifact storage unavailable")
    }
    async fn configure_mcp(&self, _session: &str, servers: &Value) -> Result<()> {
        anyhow::ensure!(
            servers.as_array().is_some_and(Vec::is_empty),
            "MCP unavailable"
        );
        Ok(())
    }
    async fn mcp_list(&self, _params: &Value, _cancel: &CancellationToken) -> Result<Value> {
        anyhow::bail!("MCP unavailable")
    }
    /// WebFetch 的抓取阶段（网络、缓存、正文抽取）；模型处理由会话侧完成。见 docs/specs/rust-webfetch.md。
    /// 系统提示词 Shell 名（TS 会话 shell 的 display.name）；None 沿用上下文默认。
    async fn shell_display_name(&self, _sink: &EventSink) -> Option<String> {
        None
    }
    async fn web_fetch(
        &self,
        _args: &Value,
        _cancel: &CancellationToken,
    ) -> Result<crate::WebFetchPage> {
        anyhow::bail!("WebFetch unavailable")
    }
    async fn scoped_definitions(
        &self,
        _session: &str,
        _cancel: &CancellationToken,
    ) -> Result<Vec<Value>> {
        Ok(self.definitions())
    }
    fn concurrent_safe_scoped(&self, _session: &str, name: &str) -> bool {
        self.concurrent_safe(name)
    }
    async fn evict_session(&self, session: &str) -> Result<()> {
        self.close_session(session).await
    }
    /// 按配置解析项目记忆根（创建目录并读取 MEMORY.md）；配置关闭时 None。见 rust-project-memory.md。
    async fn project_memory(&self) -> Option<crate::ProjectMemory> {
        None
    }
    /// 记忆目录 manifest（mtime 倒序前 200 个 `.md`，不含 MEMORY.md）。
    async fn memory_manifest(&self, _root: &str) -> Vec<zcode_cli_domain::memory::ManifestEntry> {
        vec![]
    }
    /// 会话的记忆写入上下文：Write/Edit 对记忆根内 `.md` 补写 originSessionId；`inherit` 复制其读取状态，
    /// `seed` 记为已完整读取（MEMORY.md 已注入上下文）。
    async fn memory_context(
        &self,
        _session: &str,
        _root: &str,
        _origin: &str,
        _inherit: Option<&str>,
        _seed: Option<&str>,
    ) {
    }
    /// 按本轮模型的输入能力调整工具面并记录（PDF 能力改变 Read 的 schema 与执行分支，rust-media-read.md）。
    async fn adapt_to_model(&self, _session: &str, _definitions: &mut [Value], _input: &Value) {}
    /// 带运行时 git 上下文的只读 Bash 分类（记忆提取的工具策略使用）。
    fn readonly_bash(&self, command: &str) -> bool {
        zcode_cli_domain::bash_policy::is_readonly(command)
    }
    /// 自定义 slash 命令展开（docs/specs/rust-custom-commands.md）；None 表示按原文发送。
    async fn resolve_command(
        &self,
        _session: Option<&str>,
        _text: &str,
        _cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        Ok(None)
    }
    /// 协议 `slashCommands` 目录（内置段 + 自定义命令）。
    async fn slash_commands(&self, _cancel: &CancellationToken) -> Vec<Value> {
        zcode_cli_domain::custom_command::builtin_catalog()
    }
    async fn discover_skills(
        &self,
        _cancel: &CancellationToken,
    ) -> Result<zcode_cli_domain::skills::SkillCatalog> {
        Ok(Default::default())
    }
    async fn load_skill(
        &self,
        _skill: &zcode_cli_domain::skills::Skill,
        _name: &str,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        anyhow::bail!("Skill loading unavailable")
    }
    fn definitions(&self) -> Vec<Value>;
    fn requires_permission(&self, name: &str) -> bool;
    /// 工具的权限能力（只读/破坏性/风险级别/作用域等）；缺省表示未知，按需确认处理。
    fn permission_capability(
        &self,
        _name: &str,
        _input: &Value,
    ) -> Option<zcode_cli_domain::permission::Capability> {
        None
    }
    fn concurrent_safe(&self, _name: &str) -> bool {
        false
    }
    async fn execute(
        &self,
        name: &str,
        arguments: &Value,
        cancel: &CancellationToken,
    ) -> Result<String>;
    async fn execute_scoped(
        &self,
        name: &str,
        arguments: &Value,
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let _ = sink;
        Ok(ToolOutput::text(
            self.execute(name, arguments, cancel).await?,
        ))
    }
    /// MCP 工具调用：带模型 tool call id（超预算图片 artifact 文件名与 TS 相同）。
    async fn execute_mcp(
        &self,
        name: &str,
        arguments: &Value,
        call_id: &str,
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let _ = call_id;
        self.execute_scoped(name, arguments, sink, cancel).await
    }
    async fn cancel_session(&self, _session: &str, _task: Option<&str>) -> Result<()> {
        Ok(())
    }
    async fn close_session(&self, session: &str) -> Result<()> {
        self.cancel_session(session, None).await
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
pub use super::tool_output::{ToolControl, ToolOutput};
pub use super::environment_ports::{AuthPort, Clock, ContextPort, RuntimeClock};
pub struct RuntimePorts {
    pub context: Arc<dyn ContextPort>,
    pub store: Arc<dyn SessionStore>,
    pub model: Option<Arc<dyn ModelPort>>,
    pub tools: Arc<dyn ToolPort>,
    pub clock: Arc<dyn RuntimeClock>,
}
