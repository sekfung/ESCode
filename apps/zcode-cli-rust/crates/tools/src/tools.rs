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
    fn mcp_tool(&self, session: &str, name: &str) -> Option<crate::contract::McpTool> { self.mcp.tool(session, name) }
    async fn background_bash_output(&self, session: &str, work_id: &str) -> Value { self.shell.output(session, work_id).await }
    async fn workflow_query(&self, method: &str, params: &Value) -> Result<Value> { self.workflow_host.request("v4.query", serde_json::json!({ "method": method, "params": params })).await }
    fn attach_host(&self, host: crate::contract::EventSink) { self.workflow_host.attach_host(host.clone()); self.mcp.attach_host(host); }
    async fn run_hook(&self, session: &str, input: Value, call_id: Option<&str>) -> Option<Value> { self.run_hook_inner(session, input, call_id).await }
    async fn workspace_hooks(&self, method: &str, params: Value) -> Result<Value> { self.workflow_host.request(method, params).await }
    async fn execute_workflow(
        &self,
        session: &str,
        call_id: &str,
        name: &str,
        args: &Value,
        selection: &Value,
        cancel: &CancellationToken,
    ) -> Option<Result<ToolOutput>> {
        if super::workflow_tools::ACTOR_TOOLS.contains(&name) {
            return Some(super::workflow_tools::execute_actor_tool(self, session, call_id, name, args).await);
        }
        if !super::workflow_host::routes(name, args) {
            return None;
        }
        Some(super::workflow_tools::execute(self, session, call_id, name, args, selection, cancel).await)
    }
    async fn workflow_actor_event(&self, actor_session: &str, event: Value) {
        self.workflow_host.notify("actor.event", serde_json::json!({ "actorSession": actor_session, "event": event }));
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
        let dir = self.artifacts.join(format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(session.as_bytes())
        ));
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("output.txt"); // TS createSubagentLifecycle 的文件名
        let temp = dir.join("output.txt.tmp");
        tokio::fs::write(&temp, text).await?;
        tokio::fs::rename(temp, &path).await?;
        Ok(path.to_string_lossy().into_owned())
    }
    async fn configure_mcp(&self, session: &str, servers: &Value) -> Result<()> {
        self.mcp.configure(session, servers)
    }
    async fn mcp_list(&self, params: &Value, cancel: &CancellationToken) -> Result<Value> {
        self.mcp.list(params, cancel).await
    }
    /// 会话 shell 的显示名（与 Bash 执行使用同一选择与 Host 偏好）。
    async fn shell_display_name(&self, sink: &crate::contract::EventSink) -> Option<String> {
        let over = super::tool_shell::shell_override(Some(sink)).await;
        let env: Vec<(String, String)> = std::env::vars().collect();
        let selection = crate::shell_select::resolve(
            crate::shell_select::Platform::current(),
            &env,
            over.as_ref(),
            &|p| std::path::Path::new(p).is_file(),
        );
        Some(crate::shell_select::display_name(&selection))
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
    /// 权限配置：与 TS 同源（`~/.zcode/cli/config.json` + 项目 zcode.json/.zcode/config.json，
    /// 合并后取 `permission` 段）。CLI 的 --allowed-tools/--disallowed-tools 在 TS 侧也投影到这段。
    async fn permission_config(&self) -> crate::domain::permission::Config {
        super::extension_config::permission_config(&self.cwd).await
    }
    async fn subagents_enabled(&self) -> bool {
        let config = super::extension_config::load(&self.cwd)
            .await
            .unwrap_or_else(|_| serde_json::json!({}));
        config["features"]["subagent"] != false
    }
    /// `plugins/list`（docs/specs/rust-plugins.md 第 1 期）：workspace 级、无会话。
    async fn plugin_list(&self, params: &Value) -> Result<Value> {
        let cwd = super::plugin_list::workspace_path(params)?;
        let layers = super::plugin_list::layers_for(&cwd, params).await?;
        super::plugin_list::list(&cwd, &layers, &CancellationToken::new()).await
    }
    /// `plugins/setEnabled`（docs/specs/rust-plugins.md 第 2 期）。
    async fn plugin_set_enabled(&self, params: &Value) -> Result<Value> {
        super::plugin_list::set_enabled(params, &CancellationToken::new()).await
    }
    /// `plugins/configure` / `plugins/resetConfig`（docs/specs/rust-plugins.md 第 4 期选项面）。
    async fn plugin_configure(&self, params: &Value, raw_params: Option<&str>) -> Result<Value> {
        super::plugin_config::configure(params, raw_params, &CancellationToken::new()).await
    }
    async fn plugin_reset_config(&self, params: &Value) -> Result<Value> {
        super::plugin_config::reset(params).await
    }
    /// 插件写面的慢操作（docs/specs/rust-plugin-marketplace-write.md W1b–W5）。
    async fn plugin_operation(
        &self,
        method: &str,
        params: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        match method {
            "plugins/install" => super::plugin_install::install(params, cancel).await,
            "plugins/update" => super::plugin_install::update(params, cancel).await,
            "plugins/describe" => super::plugin_describe::describe(params).await,
            "plugins/validate" => super::plugin_validate::validate(params).await,
            "plugins/marketplace/add" => {
                super::plugin_market_write::add_params(params, cancel).await
            }
            "plugins/marketplace/remove" => super::plugin_market_write::remove_params(params).await,
            "plugins/marketplace/update" => {
                super::plugin_market_write::update_params(params, cancel).await
            }
            _ => anyhow::bail!("Unsupported plugin operation: {method}"),
        }
    }
    async fn prepare_tool(
        &self,
        session: &str,
        name: &str,
        args: &Value,
        skill_loaded: bool,
    ) -> Result<Option<std::result::Result<(Value, bool), String>>> {
        let (cwd, analyzer) = (&self.workspace_path, &self.analyzer);
        match name {
            super::save_workflow::TOOL => {
                super::save_workflow::prepare(cwd, args, skill_loaded, analyzer).await
            }
            super::eval_workflow_snippet::TOOL => {
                super::eval_workflow_snippet::prepare(cwd, args, skill_loaded, analyzer).await
            }
            name if super::workflow_host::TOOLS.contains(&name) => {
                return super::workflow_tools::prepare(self, session, name, args, skill_loaded)
                    .await
                    .map(Some);
            }
            _ => return Ok(None),
        }
        .map(Some)
    }
    async fn plugin_suggested_reference(
        &self,
        params: &Value,
        refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<Value>> {
        super::plugin_suggested::resolve(params, refresh, cancel).await
    }
    /// `plugins/uninstall` / `plugins/restoreBuiltin`（docs/specs/rust-plugin-marketplace-write.md W1a）。
    async fn plugin_uninstall(&self, params: &Value) -> Result<Value> {
        super::plugin_uninstall::uninstall(params, &CancellationToken::new()).await
    }
    async fn plugin_restore_builtin(&self, params: &Value) -> Result<Value> {
        super::plugin_uninstall::restore_builtin(params).await
    }
    /// `plugins/overview`（docs/specs/rust-plugins.md 第 3 期）。
    async fn plugin_overview(&self, params: &Value) -> Result<Value> {
        super::plugin_overview::overview(params, &CancellationToken::new()).await
    }
    /// `plugins/referenceCatalog(WithCategory)`（docs/specs/rust-plugins.md 第 3 期）。
    async fn plugin_reference_catalog(
        &self,
        params: &Value,
        frozen: Option<&Value>,
        include_category: bool,
    ) -> Result<Value> {
        let cancel = CancellationToken::new();
        super::plugin_reference::catalog(params, frozen, include_category, &cancel).await
    }
    async fn discover_skills(
        &self,
        cancel: &CancellationToken,
    ) -> Result<crate::domain::skills::SkillCatalog> {
        super::tool_skills::discover(&self.cwd, cancel).await
    }
    /// 已保存工作流的 GUI 中枢（docs/specs/rust-dynamic-workflow.md 第 2 期）。
    async fn saved_workflow_op(&self, op: &str, params: &Value) -> Result<Value> {
        super::saved_workflows_hub::op(op, params)
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
    /// 能力表来自 TS 工具元数据（生成资产，见 scripts/generate-zcode-cli-rust-tool-schemas.mjs）。
    fn permission_capability(
        &self,
        name: &str,
        input: &Value,
    ) -> Option<zcode_cli_domain::permission::Capability> {
        super::tool_capability::capability(&self.cwd, name, input)
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
        check_cancel(cancel)?;
        let artifacts = super::mcp_connection::ImageArtifacts {
            root: &self.artifacts,
            session: &sink.session_id,
            call_id,
        };
        self.mcp
            .call(&sink.session_id, name, args, meta, Some(artifacts), cancel)
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
