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
pub use crate::failures::{ProcessCleanupFailure, StorageCommitFailure, ToolHandlerFailure};

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
    Event, EventSink, HOST_CHANNEL, HostReply, Input, ModelOutput, PermissionOutcome, RunEvent,
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
    /// `workflows/runs` 的 journal 读面（docs/specs/rust-dynamic-workflow.md 第 4 期前置）。
    /// 缺省表示这个 store 没有 dwf journal——TS 在 journal 缺席时同样回空页而不是报错。
    async fn workflow_runs(
        &self,
        _query: &zcode_cli_domain::dwf_journal::RunQuery,
    ) -> Result<Vec<zcode_cli_domain::dwf_journal::JournalRun>> {
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
    /// `ListModels` 的目录面（TS `createModelCatalogPort`，docs/specs/rust-dynamic-workflow.md 第 6 期）：
    /// providerId/modelId/providerLabel?/reasoningLevels/defaultReasoningLevel?/contextWindow?。
    fn model_catalog(&self) -> Vec<Value> {
        Vec::new()
    }
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
    /// 成功轮次收尾：本轮用过浏览器时对 active tab 截图，返回 `browser_turn_end` 图片卡；默认无。
    async fn browser_turn_screenshot(&self, _session: &str, _turn: &str) -> Option<Value> {
        None
    }
    /// 任一 turn 收尾：浏览器等会话资源的轮次生命周期（TS BrowserControlPort.turnEnded）；默认无操作。
    async fn turn_ended(&self, _session: &str, _turn: &str) {}
    /// 会话中 MCP 工具的元数据（ToolStart 时的工具卡）；默认无。
    fn mcp_tool(&self, _session: &str, _name: &str) -> Option<McpTool> {
        None
    }
    /// Engine 启动时交给工具层的 Host 通道（`HOST_CHANNEL`）；默认不使用。
    fn attach_host(&self, _host: EventSink) {}
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
    /// WebFetch 的抓取阶段（网络、缓存、正文抽取、超预算正文的 artifact）；模型处理由会话侧完成。
    /// `session`/`call_id` 只用于 artifact 文件名与 URI（与 TS `writeToolResultArtifact` 同形）。
    /// 见 docs/specs/rust-webfetch.md。
    /// 系统提示词 Shell 名（TS 会话 shell 的 display.name）；None 沿用上下文默认。
    async fn shell_display_name(&self, _sink: &EventSink) -> Option<String> {
        None
    }
    async fn web_fetch(
        &self,
        _args: &Value,
        _session: &str,
        _call_id: &str,
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
    /// 权限相关的用户配置（TS `PermissionService` 构造参数）：`permission.allowedTools` /
    /// `disallowedTools` / `autoApproveHighRisk`。与 TS 一样在启动时读取一次。
    async fn permission_config(&self) -> zcode_cli_domain::permission::Config {
        zcode_cli_domain::permission::Config::default()
    }
    /// `features.subagent`：false 时 TS 不注入 SubagentPort，Agent/SendMessage 既不进工具面，
    /// 直接调用也报 ConfigurationError（docs/specs/rust-subagents.md）。与 TS 一样启动读一次。
    async fn subagents_enabled(&self) -> bool {
        true
    }
    /// `plugins/list`（docs/specs/rust-plugins.md 第 1 期）：workspace 级、无会话；
    /// 参数为协议原样（`workspace` / `configScope`）。
    async fn plugin_list(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/setEnabled`（docs/specs/rust-plugins.md 第 2 期）：写 user/workspace 配置的
    /// `plugins.enabledPlugins[id]`，参数为协议原样（`workspace` / `pluginId` / `enabled` / `scope`）。
    async fn plugin_set_enabled(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/configure` / `plugins/resetConfig`（docs/specs/rust-plugins.md 第 4 期选项面）。
    async fn plugin_configure(&self, _params: &Value, _raw_params: Option<&str>) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    async fn plugin_reset_config(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/marketplace/add|remove|update`（docs/specs/rust-plugin-marketplace-write.md W4）；`op` 为末段。
    async fn plugin_marketplace(&self, _op: &str, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/install`（docs/specs/rust-plugin-marketplace-write.md W1b：本地源）。
    async fn plugin_install(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/uninstall` / `plugins/restoreBuiltin`（docs/specs/rust-plugin-marketplace-write.md W1a）。
    async fn plugin_uninstall(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    async fn plugin_restore_builtin(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/overview`（docs/specs/rust-plugins.md 第 3 期）：市场/目录/已安装/可恢复内置的读面。
    async fn plugin_overview(&self, _params: &Value) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    /// `plugins/referenceCatalog(WithCategory)`（第 3 期）：返回 `{identity, plugins}`；`frozen` 是会话冻结的
    /// 身份条目（为空时现算并在 `identity` 里交回给 engine 冻结）。
    async fn plugin_reference_catalog(
        &self,
        _params: &Value,
        _frozen: Option<&Value>,
        _include_category: bool,
    ) -> Result<Value> {
        anyhow::bail!("Plugins unavailable")
    }
    async fn discover_skills(
        &self,
        _cancel: &CancellationToken,
    ) -> Result<zcode_cli_domain::skills::SkillCatalog> {
        Ok(Default::default())
    }
    /// 已保存工作流的 GUI 中枢（`workflows/list|get|updateMeta|delete|move`，workspace 级、无会话；
    /// docs/specs/rust-dynamic-workflow.md 第 2 期）。`workflows/runs` 需要 journal（第 4 期）。
    async fn saved_workflow_op(&self, op: &str, _params: &Value) -> Result<Value> {
        anyhow::bail!("Unsupported saved workflow operation: {op}")
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
        meta: &Value,
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let _ = (call_id, meta);
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
pub use super::tool_output::{McpTool, ToolControl, ToolOutput};
pub use super::environment_ports::{AuthPort, Clock, ContextPort, RuntimeClock};
pub struct RuntimePorts {
    pub context: Arc<dyn ContextPort>,
    pub store: Arc<dyn SessionStore>,
    pub model: Option<Arc<dyn ModelPort>>,
    pub tools: Arc<dyn ToolPort>,
    pub clock: Arc<dyn RuntimeClock>,
}
