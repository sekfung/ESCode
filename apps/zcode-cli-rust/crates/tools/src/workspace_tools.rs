//! 工作区工具集的状态与工具分派（内置工具按名派发；`ToolPort` 实现在 tools.rs）。
use super::tool_args::check_cancel;
use super::{
    tool_files::{FileState, FileTools},
    tool_shell::ShellTasks,
};
use crate::contract::{EventSink, ToolOutput};
use anyhow::{Result, bail};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct WorkspaceTools {
    pub(super) cwd: PathBuf,
    /// Host 提交的 workspace 绝对路径（不解析符号链接/短文件名），记忆根按它哈希，与 Node 一致。
    pub(super) workspace_path: PathBuf,
    pub(super) artifacts: PathBuf,
    pub(super) reads: Mutex<HashMap<String, Arc<Mutex<FileState>>>>,
    /// 会话的记忆上下文（记忆根, 来源会话），Write/Edit 据此补写 originSessionId。
    pub(super) memory: Mutex<HashMap<String, (String, String)>>,
    /// 会话本轮模型的 inputFormat（Read 的 PDF/图片分支据此判定）。
    pub(super) models: Mutex<HashMap<String, Value>>,
    // File writes from different sessions share one commit gate; reads remain concurrent.
    pub(super) writes: Arc<Mutex<()>>,
    pub(super) shell: ShellTasks,
    pub(super) mcp: super::mcp_hub::Hub,
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
    pub(super) async fn call_inner(
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
            name if name.starts_with("mcp__") => {
                self.mcp
                    .call(session, name, args, &Value::Null, None, cancel)
                    .await
            }
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
                    // TS resolveWorkspacePath 以词法 workingDirectory 为基准（不 realpath），模型可见的路径与
                    // 「current working directory」文案同 Node；读写状态键仍在 tool_files 内 realpath。
                    cwd: &self.workspace_path,
                    artifacts: &artifacts,
                    state: &state,
                    writes: &self.writes,
                };
                files.call(name, args, cancel).await
            }
            "Glob" | "Grep" => super::tool_search::search(&self.cwd, name, args, cancel).await,
            // Kept for existing native transcripts, but no longer advertised to the model.
            "List" => super::tool_search::list(&self.cwd, args, cancel).await,
            // 已保存工作流清单（docs/specs/rust-dynamic-workflow.md 第 2 期）：cwd 恒取会话工作目录，
            // 模型无权跨项目扫盘（TS handler 同），这也是 `sideEffectScope: "none"` 成立的前提。
            "ListSavedWorkflows" => {
                let home =
                    std::path::PathBuf::from(zcode_cli_host::credential_cipher::node_homedir());
                let listed = super::saved_workflows::list(&self.workspace_path, &home, None);
                let mut output = ToolOutput::new(
                    super::saved_workflows::model_content(&listed),
                    super::saved_workflows::to_value(&super::saved_workflows::output(&listed)),
                );
                output.display = Some(super::saved_workflows::to_value(
                    &super::saved_workflows::display(&listed),
                ));
                Ok(output)
            }
            "Bash" | "TaskOutput" | "TaskStop" => {
                let started = std::time::SystemTime::now();
                let paths = (
                    self.cwd.as_path(),
                    artifacts.as_path(),
                    self.workspace_path.as_path(),
                );
                let mut output = self
                    .shell
                    .call(paths, session, name, args, sink, cancel)
                    .await?;
                if name == "Bash" {
                    let state = self
                        .reads
                        .lock()
                        .await
                        .entry(session.to_owned())
                        .or_default()
                        .clone();
                    let command = args["command"].as_str().unwrap_or_default();
                    super::bash_read_state::apply(
                        &state,
                        &self.workspace_path,
                        &mut output,
                        command,
                        started,
                    )
                    .await;
                }
                Ok(output)
            }
            _ => bail!("Unsupported tool: {name}"),
        }
    }
}
