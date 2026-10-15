//! `escode-task` channel 的共用件：懒打开的索引句柄、查询上下文、运行时转发句柄与参数取值。
//! 从 `domains_task.rs` 拆出以守住单文件行数上限。

use std::path::{Path, PathBuf};

use serde_json::Value;
use std::sync::{Arc, Mutex};

use escode_host_services::tasks_index::TasksIndex;

use crate::domains_file::spawn_blocking_io;
use crate::task_targets::TaskTargets;
use crate::wire::Wire;

/// TS `ESCODE_AGENT_PROVIDER`：任务列表只接受当前 glm provider 的行。
pub const GLM_PROVIDER: &str = "glm";

/// TS `getAppConfigDir()` = `<dataBaseDir>/.escode/v2`，索引文件名 `tasks-index.sqlite`。
pub fn tasks_index_path(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite")
}

/// 懒打开的索引句柄（失败不缓存，下次调用重试）。
#[derive(Default)]
pub struct LazyIndex {
    index: Option<TasksIndex>,
}

impl LazyIndex {
    pub fn with<T>(
        &mut self,
        path: &Path,
        read: impl FnOnce(&TasksIndex) -> Result<T, String>,
    ) -> Result<T, (String, String)> {
        if self.index.is_none() {
            self.index = Some(TasksIndex::open(path).map_err(|error| {
                // 结构化错误：Main/Renderer 可据此区分「未建库」「账本不符」。
                ("TasksIndexUnavailable".to_owned(), error.to_string())
            })?);
        }
        let index = self.index.as_ref().expect("index opened above");
        read(index).map_err(|error| ("TasksIndexUnavailable".to_owned(), error))
    }
}

/// 调用上下文：索引路径 + 懒句柄 + 参数。
#[derive(Clone)]
pub struct ReadContext {
    pub path: PathBuf,
    pub index: Arc<Mutex<LazyIndex>>,
    /// 任务 → workspace 目标注册表（只有 `{taskId}` 入参的方法用它）。
    pub targets: Arc<TaskTargets>,
    /// 运行时转发（`session/read` 等；任务域只有少数方法需要）。
    pub runtime: Option<RuntimeAccess>,
}

/// 转发到 workspace 实例所需的句柄（与 session channel 同一套装配）。
#[derive(Clone)]
pub struct RuntimeAccess {
    pub instances: escode_host_runtime::Instances,
    pub options: escode_host_surface::OptionsFactory,
}

impl ReadContext {
    /// 在阻塞线程里取锁并执行一次索引读。
    pub async fn run<T, F>(&self, read: F) -> Result<T, (String, String)>
    where
        T: Send + 'static,
        F: FnOnce(&TasksIndex) -> Result<T, String> + Send + 'static,
    {
        let index = Arc::clone(&self.index);
        let path = self.path.clone();
        spawn_blocking_io(move || {
            let mut guard = index.lock().map_err(|_| {
                (
                    "TasksIndexUnavailable".to_owned(),
                    "tasks-index lock poisoned".to_owned(),
                )
            })?;
            guard.with(&path, read)
        })
        .await
    }
}

/// 借出上下文（闭包捕获需要 `'static`，这里只取字段的引用副本）。
pub fn context_ref(context: &ReadContext) -> ReadContext {
    ReadContext {
        path: context.path.clone(),
        index: Arc::clone(&context.index),
        targets: Arc::clone(&context.targets),
        runtime: context.runtime.clone(),
    }
}

/// 参数取值：`workspacePath` 缺省为空串（`listPinnedTaskIds` 不传）。
pub fn workspace_path_of(params: &Value) -> String {
    params["workspacePath"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// `workspaceIdentity` 空白视同未提供（AGENTS.md Workspace Identity）。
pub fn workspace_identity_of(params: &Value) -> Option<String> {
    params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// RPC 实参：`ProxyChannel.toService` 把 JS 实参打包成数组，取第一个并当对象用。
pub fn first_object(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}
