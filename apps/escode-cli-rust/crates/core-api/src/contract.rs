//! Public runtime ports. The application owns state; adapters own external IO.
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
pub use escode_cli_domain::model::{ModelFailure, RetryState};

#[allow(unused_imports)]
pub use super::contract_model::{ModelPort, ModelRegistry, RewindTransaction};
#[allow(unused_imports)]
pub use super::contract_store::{DurableCommitReceipt, SessionStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub provider_id: String,
    pub model_id: String,
    pub reasoning_level: String,
}

pub type Output = mpsc::Sender<Vec<Value>>;

pub struct ChildHandle {
    pub task: escode_cli_domain::subagent::Task,
    pub updates: tokio::sync::watch::Receiver<escode_cli_domain::subagent::Task>,
    pub message_id: Option<String>,
    pub delivery: Option<String>,
}
pub use crate::failures::{ProcessCleanupFailure, StorageCommitFailure, ToolHandlerFailure};

pub use crate::contract_events::{
    Event, EventSink, HOST_CHANNEL, HostReply, Input, ModelOutput, PermissionOutcome, RunEvent,
};

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
        _changes: &[escode_cli_domain::file_checkpoint::FileCheckpoint],
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
        _changes: &[escode_cli_domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Value> {
        anyhow::bail!("File rewind unavailable")
    }
    async fn begin_rewind(
        &self,
        _session: &str,
        _token: &str,
        _changes: &[escode_cli_domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Box<dyn RewindTransaction>> {
        anyhow::bail!("File rewind unavailable")
    }
    async fn agent_memory(
        &self,
        _profile: &escode_cli_domain::subagent::Profile,
        _cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        Ok(None)
    }
    async fn agent_profiles(
        &self,
        _cancel: &CancellationToken,
    ) -> Result<Vec<escode_cli_domain::subagent::Profile>> {
        Ok(escode_cli_domain::subagent::builtins())
    }
    async fn inherit_session(&self, _parent: &str, _child: &str) -> Result<()> {
        Ok(())
    }
    /// 批准的计划写入 `<workspace>/.escode/plans/<file_name>`；无文件系统时与 TS 一样跳过。
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
    /// 恢复会话首轮的 shell 提醒正文（docs/specs/rust-shell-resume-notice.md）；`persisted` 是会话起始时写进
    /// 提示词的 Shell 名。None 表示无需提醒。
    async fn shell_resume_notice(
        &self,
        _sink: &EventSink,
        _persisted: Option<&str>,
    ) -> Option<String> {
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
    async fn memory_manifest(&self, _root: &str) -> Vec<escode_cli_domain::memory::ManifestEntry> {
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
        escode_cli_domain::bash_policy::is_readonly(command)
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
        escode_cli_domain::custom_command::builtin_catalog()
    }
    /// 权限相关的用户配置（TS `PermissionService` 构造参数）：`permission.allowedTools` /
    /// `disallowedTools` / `autoApproveHighRisk`。与 TS 一样在启动时读取一次。
    async fn permission_config(&self) -> escode_cli_domain::permission::Config {
        escode_cli_domain::permission::Config::default()
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
    /// 插件写面的慢操作（`plugins/install|update`、`plugins/marketplace/add|remove|update`；
    /// docs/specs/rust-plugin-marketplace-write.md）。`cancel` 由 `plugins/cancelOperation` 置位，操作在安全点检查。
    async fn plugin_operation(
        &self,
        method: &str,
        _params: &Value,
        _cancel: &CancellationToken,
    ) -> Result<Value> {
        anyhow::bail!("Unsupported plugin operation: {method}")
    }
    /// 工具的执行前预处理（TS validateInput → resolveInput → prepareApproval；目前只有 SaveWorkflow）。
    /// `None`：该工具没有预处理；`Some(Err(文案))`：交回模型的工具失败，不请求权限；
    /// `Some(Ok((入参, ask)))`：归一化后的入参，以及是否需要确认（`false` = ask 时直接放行）。
    async fn prepare_tool(
        &self,
        _session: &str,
        _name: &str,
        _args: &Value,
        _skill_loaded: bool,
    ) -> Result<Option<std::result::Result<(Value, bool), String>>> {
        Ok(None)
    }
    /// 工作流宿主负责的工具（CreateWorkflow 等，docs/specs/rust-dynamic-workflow.md M1）：带会话与调用 id 执行，
    /// 后台 run 结算后宿主经 Host 通道报告 `Event::WorkflowSettled`。`None`：该工具不归宿主。
    async fn execute_workflow(
        &self,
        _session: &str,
        _call_id: &str,
        _name: &str,
        _args: &Value,
        _selection: &Value,
        _cancel: &CancellationToken,
    ) -> Option<Result<crate::ToolOutput>> {
        None
    }
    /// actor 会话的运行事件（工具开始 / 结束、模型请求），转交工作流宿主合成 TS SessionEvent。
    async fn workflow_actor_event(&self, _actor_session: &str, _event: Value) {}
    /// `v4/conversation/backgroundBashOutput`：后台 Bash 的输出尾窗；无此任务回 `unavailable`。
    async fn background_bash_output(&self, _session: &str, work_id: &str) -> Value {
        serde_json::json!({ "kind": "unsupported", "workId": work_id })
    }
    /// Hooks（docs/specs/rust-hooks.md）：执行一次 hook 事件，返回 TS HookRunResult；本会话没有 hooks 时 None。
    async fn run_hook(&self, _session: &str, _input: Value, _call_id: Option<&str>) -> Option<Value> {
        None
    }
    /// 工作区 hooks 的审核命令与无会话授权（H2）：转给工作流宿主（`hooks.review` / `hooks.trustGrant`）。
    async fn workspace_hooks(&self, method: &str, _params: Value) -> Result<Value> {
        anyhow::bail!("Unsupported workspace hooks method: {method}")
    }
    /// V4 工作流只读查询（`v4/conversation/workflowRun*`，方法名去掉前缀）：由工作流宿主按 Node 网关应答。
    async fn workflow_query(&self, method: &str, _params: &Value) -> Result<Value> {
        anyhow::bail!("Unsupported workflow query: {method}")
    }
    /// 用户命令面的工作流 run 操作（`run.cancel` / `run.resume` …，docs/specs/rust-v4-command-gaps.md）：
    /// 交给工作流宿主执行，返回宿主的结构化结果（`ok` + `reason` / `runId`）。
    async fn workflow_run(&self, method: &str, _params: Value) -> Result<Value> {
        anyhow::bail!("Unsupported workflow run command: {method}")
    }
    /// `plugins/resolveSuggestedReference` 两段式：`refresh = false` 只查本地（未命中返回 None），
    /// `refresh = true` 刷新官方目录后给出最终结果。
    async fn plugin_suggested_reference(
        &self,
        _params: &Value,
        _refresh: bool,
        _cancel: &CancellationToken,
    ) -> Result<Option<Value>> {
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
    ) -> Result<escode_cli_domain::skills::SkillCatalog> {
        Ok(Default::default())
    }
    /// 已保存工作流的 GUI 中枢（`workflows/list|get|updateMeta|delete|move`，workspace 级、无会话；
    /// docs/specs/rust-dynamic-workflow.md 第 2 期）。`workflows/runs` 需要 journal（第 4 期）。
    async fn saved_workflow_op(&self, op: &str, _params: &Value) -> Result<Value> {
        anyhow::bail!("Unsupported saved workflow operation: {op}")
    }
    async fn load_skill(
        &self,
        _skill: &escode_cli_domain::skills::Skill,
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
    ) -> Option<escode_cli_domain::permission::Capability> {
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
pub use super::environment_ports::{AuthPort, Clock, ContextPort, RuntimeClock};
pub use super::tool_output::{McpTool, ToolControl, ToolOutput};

pub struct RuntimePorts {
    pub context: Arc<dyn ContextPort>,
    pub store: Arc<dyn SessionStore>,
    pub model: Option<Arc<dyn ModelPort>>,
    pub tools: Arc<dyn ToolPort>,
    pub clock: Arc<dyn RuntimeClock>,
}
