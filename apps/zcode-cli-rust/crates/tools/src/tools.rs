use super::{
    tool_files::{FileState, FileTools},
    tool_shell::ShellTasks,
};
use crate::contract::{EventSink, ToolOutput, ToolPort};
use anyhow::{Result, bail};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct WorkspaceTools {
    cwd: PathBuf,
    /// Host 提交的 workspace 绝对路径（不解析符号链接/短文件名），记忆根按它哈希，与 Node 一致。
    workspace_path: PathBuf,
    artifacts: PathBuf,
    reads: Mutex<HashMap<String, Arc<Mutex<FileState>>>>,
    /// 会话的记忆上下文（记忆根, 来源会话），Write/Edit 据此补写 originSessionId。
    memory: Mutex<HashMap<String, (String, String)>>,
    /// 会话本轮模型的 inputFormat（Read 的 PDF/图片分支据此判定）。
    models: Mutex<HashMap<String, Value>>,
    // File writes from different sessions share one commit gate; reads remain concurrent.
    writes: Arc<Mutex<()>>,
    shell: ShellTasks,
    mcp: super::mcp_hub::Hub,
}
impl WorkspaceTools {
    pub fn new(cwd: PathBuf, artifacts: PathBuf) -> Self {
        Self {
            mcp: super::mcp_hub::Hub::new(cwd.clone()),
            workspace_path: cwd.clone(),
            cwd,
            artifacts,
            reads: Mutex::new(HashMap::new()),
            memory: Mutex::new(HashMap::new()),
            models: Mutex::new(HashMap::new()),
            writes: Arc::new(Mutex::new(())),
            shell: ShellTasks::default(),
        }
    }
    /// 修复：记忆根此前按 realpath 后的 cwd 哈希，macOS `/var`→`/private/var`、Windows 8.3 短名展开后
    /// 与 Node（按 Host 路径 resolve）不同，两侧记忆不共享；改用 Host 路径。
    pub fn with_workspace_path(mut self, path: PathBuf) -> Self {
        self.workspace_path = path;
        self
    }
    pub async fn call(
        &self,
        session: &str,
        name: &str,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        self.call_inner(session, name, args, None, cancel).await
    }
    async fn call_inner(
        &self,
        session: &str,
        name: &str,
        args: &Value,
        sink: Option<&EventSink>,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        check_cancel(cancel)?;
        if !args.is_object() {
            bail!("Tool arguments must be an object");
        }
        let artifacts = self.artifacts.join(format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(session.as_bytes())
        ));
        match name {
            name if name.starts_with("mcp__") => self.mcp.call(session, name, args, &Value::Null, None, cancel).await,
            "Read" | "Write" | "Edit" => {
                let state = self
                    .reads
                    .lock()
                    .await
                    .entry(session.to_owned())
                    .or_default()
                    .clone();
                let memory = self.memory.lock().await.get(session).cloned();
                let input_format = self.models.lock().await.get(session).cloned();
                let files = FileTools {
                    memory: memory
                        .as_ref()
                        .map(|(root, origin)| (root.as_str(), origin.as_str())),
                    input_format: input_format.unwrap_or_default(),
                    sink,
                    checkpoint_root: &self.artifacts,
                    cwd: &self.cwd,
                    artifacts: &artifacts,
                    state: &state,
                    writes: &self.writes,
                };
                files.call(name, args, cancel).await
            }
            "Glob" | "Grep" => super::tool_search::search(&self.cwd, name, args, cancel).await,
            // Kept for existing native transcripts, but no longer advertised to the model.
            "List" => super::tool_search::list(&self.cwd, args, cancel).await,
            "Bash" | "TaskOutput" | "TaskStop" => {
                self.shell
                    .call((&self.cwd, &artifacts), session, name, args, sink, cancel)
                    .await
            }
            _ => bail!("Unsupported tool: {name}"),
        }
    }
}
#[async_trait::async_trait]
impl ToolPort for WorkspaceTools {
    async fn browser_turn_screenshot(&self, session: &str, turn: &str) -> Option<Value> {
        self.mcp.browser_turn_screenshot(session, turn).await
    }
    async fn turn_ended(&self, session: &str, turn: &str) {
        self.mcp.browser_lifecycle(session, Some(turn), false).await;
    }
    fn mcp_display(&self, session: &str, name: &str) -> Option<Value> {
        self.mcp.display(session, name)
    }
    fn attach_host(&self, host: crate::contract::EventSink) {
        self.mcp.attach_host(host);
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
        let path = dir.join("agent.output");
        let temp = dir.join("agent.output.tmp");
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
        cancel: &CancellationToken,
    ) -> Result<crate::contract::WebFetchPage> {
        super::web_fetch::fetch(&super::web_fetch::HttpTransport, args, cancel).await
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
    async fn discover_skills(
        &self,
        cancel: &CancellationToken,
    ) -> Result<crate::domain::skills::SkillCatalog> {
        super::tool_skills::discover(&self.cwd, cancel).await
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
        matches!(
            name,
            "Read"
                | "WebFetch"
                | "WebSearch"
                | "ReadSessionContext"
                | "List"
                | "Glob"
                | "Grep"
                | "AskUserQuestion"
                | "TodoRead"
                | "Skill"
                | "Agent"
                | "Task"
        )
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
        self.mcp.shutdown().await
    }
}
pub(super) use super::tool_args::{boolean, keys, resolve, string, uint};
pub(super) fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("Cancelled")
    }
    Ok(())
}
pub fn truncate_utf8(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1
        }
        text.truncate(end);
    }
}
