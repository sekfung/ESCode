#[allow(unused_imports)]
pub use super::workspace_tools::WorkspaceTools;
use crate::contract::{EventSink, ToolOutput, ToolPort};
use anyhow::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
#[async_trait::async_trait]
impl ToolPort for WorkspaceTools {
    async fn browser_turn_screenshot(&self, session: &str, turn: &str) -> Option<Value> {
        self.mcp.browser_turn_screenshot(session, turn).await
    }
    async fn turn_ended(&self, session: &str, turn: &str) {
        self.mcp.browser_lifecycle(session, Some(turn), false).await;
    }
    fn mcp_tool(&self, session: &str, name: &str) -> Option<crate::contract::McpTool> {
        self.mcp.tool(session, name)
    }
    async fn background_bash_output(&self, session: &str, work_id: &str) -> Value {
        self.shell.output(session, work_id).await
    }
    async fn workflow_query(&self, method: &str, params: &Value) -> Result<Value> {
        self.workflow_host
            .request(
                "v4.query",
                serde_json::json!({ "method": method, "params": params }),
            )
            .await
    }
    fn attach_host(&self, host: crate::contract::EventSink) {
        self.workflow_host.attach_host(host.clone());
        self.mcp.attach_host(host);
    }
    async fn run_hook(&self, session: &str, input: Value, call_id: Option<&str>) -> Option<Value> {
        self.run_hook_inner(session, input, call_id).await
    }
    async fn workspace_hooks(&self, method: &str, params: Value) -> Result<Value> {
        self.workflow_host.request(method, params).await
    }
    async fn workflow_run(&self, method: &str, params: Value) -> Result<Value> {
        self.workflow_host.request(method, params).await
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_workflow(
        &self,
        session: &str,
        call_id: &str,
        name: &str,
        args: &Value,
        selection: &Value,
        mode: &str,
        cancel: &CancellationToken,
    ) -> Option<Result<ToolOutput>> {
        super::workspace_tools_misc::execute_workflow(
            self, session, call_id, name, args, selection, mode, cancel,
        )
        .await
    }
    async fn workflow_actor_event(&self, actor_session: &str, event: Value) {
        self.workflow_host.notify(
            "actor.event",
            serde_json::json!({ "actorSession": actor_session, "event": event }),
        );
    }
    async fn file_changes(
        &self,
        changes: &[crate::domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Value> {
        super::file_changes::details(&self.artifacts, changes).await
    }
    async fn pending_rewinds(&self) -> Result<Vec<String>> {
        super::file_rewind::pending(&self.artifacts).await
    }
    async fn recover_rewind(&self, session: &str, committed: Option<&str>) -> Result<()> {
        super::file_rewind::recover(&self.artifacts, session, committed, self.writes.clone()).await
    }
    async fn rewind_preview(
        &self,
        changes: &[crate::domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Value> {
        super::file_rewind::preview(&self.artifacts, changes).await
    }
    async fn begin_rewind(
        &self,
        session: &str,
        token: &str,
        changes: &[crate::domain::file_checkpoint::FileCheckpoint],
    ) -> Result<Box<dyn crate::contract::RewindTransaction>> {
        super::file_rewind::begin(
            &self.artifacts,
            session,
            token,
            changes,
            self.writes.clone(),
        )
        .await
    }

    async fn agent_memory(
        &self,
        profile: &crate::domain::subagent::Profile,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        super::agent_profiles::memory(&self.cwd, profile, cancel).await
    }
    async fn agent_profiles(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Vec<crate::domain::subagent::Profile>> {
        super::agent_profiles::discover(&self.cwd, cancel).await
    }
    async fn inherit_session(&self, parent: &str, child: &str) -> Result<()> {
        self.mcp.inherit(parent, child);
        Ok(())
    }
    async fn write_plan_file(&self, file_name: &str, plan: &str) -> Result<()> {
        super::plan_tools::write_plan_file(&self.cwd, file_name, plan).await
    }
    async fn agent_output(&self, session: &str, text: &str) -> Result<String> {
        super::workspace_tools_misc::agent_output(self, session, text).await
    }
    async fn configure_mcp(&self, session: &str, servers: &Value) -> Result<()> {
        self.mcp.configure(session, servers)
    }
    async fn mcp_list(&self, params: &Value, cancel: &CancellationToken) -> Result<Value> {
        self.mcp.list(params, cancel).await
    }
    /// 会话 shell 的显示名（与 Bash 执行使用同一选择与 Host 偏好）。
    async fn shell_display_name(&self, sink: &crate::contract::EventSink) -> Option<String> {
        Some(super::tool_shell_support::display_name(sink).await)
    }
    async fn shell_resume_notice(
        &self,
        sink: &crate::contract::EventSink,
        persisted: Option<&str>,
    ) -> Option<String> {
        super::tool_shell_support::resume_notice(sink, persisted).await
    }
    async fn web_fetch(
        &self,
        args: &Value,
        session: &str,
        call_id: &str,
        cancel: &CancellationToken,
    ) -> Result<crate::contract::WebFetchPage> {
        super::web_fetch::fetch(
            &super::web_fetch::HttpTransport,
            args,
            Some(super::web_fetch::ArtifactTarget {
                root: &self.artifacts,
                session,
                call_id,
            }),
            cancel,
        )
        .await
    }
    async fn scoped_definitions(
        &self,
        session: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<Value>> {
        let mut definitions = self.definitions();
        definitions.extend(self.mcp.definitions(session, cancel).await?);
        Ok(definitions)
    }
    fn concurrent_safe_scoped(&self, session: &str, name: &str) -> bool {
        self.concurrent_safe(name) || self.mcp.safe(session, name)
    }
    async fn evict_session(&self, session: &str) -> Result<()> {
        self.shell.close_session(session).await?;
        self.reads.lock().await.remove(session);
        self.memory.lock().await.remove(session);
        self.models.lock().await.remove(session);
        self.mcp.close_session(session, false).await
    }
    async fn resolve_command(
        &self,
        session: Option<&str>,
        text: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        super::custom_command_shell::resolve(&self.cwd, session, text, cancel).await
    }
    async fn slash_commands(&self, cancel: &CancellationToken) -> Vec<Value> {
        super::custom_command_shell::catalog(&self.cwd, cancel).await
    }
    /// 权限配置：与 TS 同源（`~/.escode/cli/config.json` + 项目 escode.json/.escode/config.json，
    /// 合并后取 `permission` 段）。CLI 的 --allowed-tools/--disallowed-tools 在 TS 侧也投影到这段。
    async fn permission_config(&self) -> crate::domain::permission::Config {
        super::workspace_tools_misc::permission_config(self).await
    }
    async fn subagents_enabled(&self) -> bool {
        let config = super::extension_config::load(&self.cwd)
            .await
            .unwrap_or_else(|_| serde_json::json!({}));
        config["features"]["subagent"] != false
    }
    async fn plugin_list(&self, params: &Value) -> Result<Value> {
        super::tool_port_plugins::list(params).await
    }
    async fn plugin_set_enabled(&self, params: &Value) -> Result<Value> {
        super::tool_port_plugins::set_enabled(params).await
    }
    async fn plugin_configure(&self, params: &Value, raw_params: Option<&str>) -> Result<Value> {
        super::tool_port_plugins::configure(params, raw_params).await
    }
    async fn plugin_reset_config(&self, params: &Value) -> Result<Value> {
        super::tool_port_plugins::reset_config(params).await
    }
    async fn plugin_operation(
        &self,
        method: &str,
        params: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        super::tool_port_plugins::operation(method, params, cancel).await
    }
    async fn prepare_tool(
        &self,
        session: &str,
        name: &str,
        args: &Value,
        skill_loaded: bool,
    ) -> Result<Option<std::result::Result<(Value, bool), String>>> {
        super::workspace_tools_misc::prepare_tool(self, session, name, args, skill_loaded).await
    }
    async fn plugin_suggested_reference(
        &self,
        params: &Value,
        refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<Value>> {
        super::tool_port_plugins::suggested_reference(params, refresh, cancel).await
    }
    async fn plugin_uninstall(&self, params: &Value) -> Result<Value> {
        super::tool_port_plugins::uninstall(params).await
    }
    async fn plugin_restore_builtin(&self, params: &Value) -> Result<Value> {
        super::tool_port_plugins::restore_builtin(params).await
    }
    async fn plugin_overview(&self, params: &Value) -> Result<Value> {
        super::tool_port_plugins::overview(params).await
    }
    async fn plugin_reference_catalog(
        &self,
        params: &Value,
        frozen: Option<&Value>,
        include_category: bool,
    ) -> Result<Value> {
        super::tool_port_plugins::reference_catalog(params, frozen, include_category).await
    }
    async fn discover_skills(
        &self,
        cancel: &CancellationToken,
    ) -> Result<crate::domain::skills::SkillCatalog> {
        super::tool_skills::discover(&self.cwd, self.include_visualize, cancel).await
    }
    /// 已保存工作流的 GUI 中枢（docs/specs/rust-dynamic-workflow.md 第 2 期）。
    async fn saved_workflow_op(&self, op: &str, params: &Value) -> Result<Value> {
        super::saved_workflows_hub::op(op, params)
    }
    async fn saved_workflow_run_op(
        &self,
        op: &str,
        params: &Value,
        run: Option<&Value>,
    ) -> Result<Value> {
        super::saved_workflows_run::op(&self.analyzer, op, params, run).await
    }
    async fn load_skill(
        &self,
        skill: &crate::domain::skills::Skill,
        name: &str,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        super::tool_skills::load(skill, name, cancel).await
    }
    fn definitions(&self) -> Vec<Value> {
        super::tool_surface::definitions()
    }
    fn requires_permission(&self, _name: &str) -> bool {
        false
    }
    /// 能力表来自 TS 工具元数据（生成资产，见 scripts/generate-escode-cli-rust-tool-schemas.mjs）。
    fn permission_capability(
        &self,
        name: &str,
        input: &Value,
    ) -> Option<escode_cli_domain::permission::Capability> {
        super::tool_capability::capability(&self.cwd, name, input)
    }
    fn guarded_rule(&self, input: &Value, shell_override: Option<&Value>) -> Option<String> {
        super::tool_capability::guarded_rule(input, shell_override)
    }
    async fn project_memory(&self) -> Option<crate::contract::ProjectMemory> {
        super::project_memory::resolve(&self.cwd, &self.workspace_path).await
    }
    async fn memory_manifest(&self, root: &str) -> Vec<crate::domain::memory::ManifestEntry> {
        super::project_memory::manifest(root).await
    }
    async fn memory_context(
        &self,
        session: &str,
        root: &str,
        origin: &str,
        inherit: Option<&str>,
        seed: Option<&str>,
    ) {
        let context = (session, root, origin, inherit, seed);
        super::project_memory::set_context(&self.memory, &self.reads, context).await;
    }
    async fn adapt_to_model(&self, session: &str, definitions: &mut [Value], input: &Value) {
        self.models
            .lock()
            .await
            .insert(session.into(), input.clone());
        if input["supportsPdf"] == true {
            super::tool_surface::apply_pdf_read(definitions);
        }
    }
    fn readonly_bash(&self, command: &str) -> bool {
        crate::bash_git_safety::is_readonly_in_context(command, Some(&self.cwd))
    }
    fn concurrent_safe(&self, name: &str) -> bool {
        super::workspace_tools::concurrent_safe(name)
    }
    async fn execute(
        &self,
        name: &str,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<String> {
        Ok(self.call("default", name, args, cancel).await?.content)
    }
    async fn execute_scoped(
        &self,
        name: &str,
        args: &Value,
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        self.call_inner(&sink.session_id, name, args, Some(sink), cancel)
            .await
    }
    async fn execute_mcp(
        &self,
        name: &str,
        args: &Value,
        call_id: &str,
        meta: &Value,
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        super::workspace_tools_misc::execute_mcp(self, name, args, call_id, meta, sink, cancel)
            .await
    }
    async fn cancel_session(&self, session: &str, task: Option<&str>) -> Result<()> {
        self.shell.cancel(session, task).await
    }
    async fn close_session(&self, session: &str) -> Result<()> {
        self.mcp.browser_lifecycle(session, None, true).await;
        self.shell.close_session(session).await?;
        self.reads.lock().await.remove(session);
        self.memory.lock().await.remove(session);
        self.models.lock().await.remove(session);
        self.mcp.close_session(session, true).await?;
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.shell.shutdown().await?;
        // 本进程创建的 shell 初始化快照随 runtime 关闭删除（TS cleanupRegistry）。
        super::shell_snapshot::cleanup().await;
        self.mcp.shutdown().await
    }
}
pub use super::tool_args::truncate_utf8;
pub(super) use super::tool_args::{boolean, check_cancel, keys, resolve, string, uint};
