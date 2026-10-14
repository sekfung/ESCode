//! `escode-task` channel（优先序 5：会话与任务域；见 docs/specs/rust-u2-tasks-index.md）。
//!
//! 本切片只接**索引读**：`getTaskMeta` 直接读 tasks-index（不依赖运行时实例）。
//! 其余方法（`createTask`/`sendPrompt`/列表类…）由 `respond` 回显式缺口，直到 S2/S3 落地。
//!
//! 索引库路径：`<dataBaseDir>/.escode/v2/tasks-index.sqlite`（TS `getTasksIndexDatabasePath`）。
//! 懒打开：TS Host 可能尚未建库，首次调用时打开失败按结构化错误回报，下次调用重试。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use escode_host_services::tasks_index::TasksIndex;

use crate::domains_file::blocking;
use crate::session::{Channel, handler};
use crate::wire::Wire;

/// TS `getAppConfigDir()` = `<dataBaseDir>/.escode/v2`，索引文件名 `tasks-index.sqlite`。
fn tasks_index_path(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite")
}

/// 懒打开的索引句柄（失败不缓存，下次调用重试）。
#[derive(Default)]
struct LazyIndex {
    index: Option<TasksIndex>,
}

impl LazyIndex {
    fn with<T>(
        &mut self,
        path: &Path,
        read: impl FnOnce(&TasksIndex) -> Result<T, (String, String)>,
    ) -> Result<T, (String, String)> {
        if self.index.is_none() {
            self.index = Some(TasksIndex::open(path).map_err(|error| {
                // 结构化错误：Main/Renderer 可据此区分「未建库」「账本不符」。
                ("TasksIndexUnavailable".to_owned(), error.to_string())
            })?);
        }
        let index = self.index.as_ref().expect("index opened above");
        read(index)
    }
}

/// `escode-task` channel。`data_base_dir` 由 Main 经 `rpcListen` 提交；缺省时显式缺口。
pub fn task_channel(data_base_dir: Option<PathBuf>) -> Channel {
    let index = Arc::new(Mutex::new(LazyIndex::default()));
    let get_task_meta = match data_base_dir {
        Some(data_base_dir) => {
            let path = tasks_index_path(&data_base_dir);
            handler(move |arg: Wire| {
                let index = Arc::clone(&index);
                let path = path.clone();
                let params = first_object(arg);
                Box::pin(async move {
                    blocking(move || {
                        let workspace_path = params["workspacePath"].as_str().unwrap_or_default();
                        let workspace_identity = params["workspaceIdentity"]
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        let task_id = params["taskId"].as_str().unwrap_or_default();
                        if task_id.is_empty() {
                            return Err((
                                "InvalidRequest".to_owned(),
                                "taskId is required.".to_owned(),
                            ));
                        }
                        let mut guard = index.lock().map_err(|_| {
                            (
                                "TasksIndexUnavailable".to_owned(),
                                "index poisoned".to_owned(),
                            )
                        })?;
                        guard.with(&path, |index| {
                            index
                                .get_task_meta(workspace_path, workspace_identity, task_id)
                                .map_err(|error| {
                                    ("TasksIndexUnavailable".to_owned(), error.to_string())
                                })
                                .map(|meta| meta.map_or(Wire::Undefined, Wire::Object))
                        })
                    })
                    .await
                })
            })
        }
        None => handler(move |_arg: Wire| {
            Box::pin(async move {
                Err((
                    "InvalidRequest".to_owned(),
                    "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.".to_owned(),
                ))
            })
        }),
    };
    Channel::new("escode-task", vec![("getTaskMeta", get_task_meta)])
}

/// RPC 实参：`ProxyChannel.toService` 把 JS 实参打包成数组，取第一个并当对象用。
fn first_object(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}
