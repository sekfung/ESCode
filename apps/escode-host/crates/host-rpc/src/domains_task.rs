//! `escode-task` channel（优先序 5：会话与任务域；见 docs/specs/rust-u2-tasks-index.md）。
//!
//! 本切片只接**索引读**：`getTaskMeta` 与列表分区（`listTasks` / `listPinnedTaskIds` /
//! `listPinnedTasks` / `listDeletedTaskIds` / `listArchivedTasks`）直读 tasks-index，
//! 不依赖运行时实例。其余方法（`createTask`/`sendPrompt`/分组写…）由 `respond` 回显式缺口。
//!
//! 过滤与排序逐条对齐 TS `escodeTaskServiceAdapter`（都走 `TaskIndexRepo.listTaskMetas`）：
//! - `listTasks`：provider=glm、pinned=false、archived=false；
//! - `listPinnedTaskIds` / `listPinnedTasks`：provider=glm、pinned=true、archived=false（前者无 workspace）；
//! - `listArchivedTasks`：provider=glm、archived=true（**不带 pinned 过滤**）；
//! - `listDeletedTaskIds`：deleted=1 且 provider=glm，按 id 升序。
//!
//! TS 侧另有进程内 overlay（`rememberIndexedTaskMeta`）记录已见 meta，不回影响返回值。
//!
//! 索引库路径：`<dataBaseDir>/.escode/v2/tasks-index.sqlite`（TS `getTasksIndexDatabasePath`）。
//! 懒打开：TS Host 可能尚未建库，首次调用时打开失败按结构化错误回报，下次调用重试。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use escode_host_services::tasks_index::{TaskFilters, TasksIndex};
use serde_json::Value;

use super::domains_task_query;
use crate::domains_file::spawn_blocking_io;
use crate::session::{Channel, handler};
use crate::wire::Wire;

/// TS `ESCODE_AGENT_PROVIDER`：任务列表只接受当前 glm provider 的行。
pub(crate) const GLM_PROVIDER: &str = "glm";

/// TS `getAppConfigDir()` = `<dataBaseDir>/.escode/v2`，索引文件名 `tasks-index.sqlite`。
fn tasks_index_path(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite")
}

/// 懒打开的索引句柄（失败不缓存，下次调用重试）。
#[derive(Default)]
pub(crate) struct LazyIndex {
    index: Option<TasksIndex>,
}

impl LazyIndex {
    pub(crate) fn with<T>(
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
pub(crate) struct ReadContext {
    pub(crate) path: PathBuf,
    pub(crate) index: Arc<Mutex<LazyIndex>>,
}

impl ReadContext {
    /// 在阻塞线程里取锁并执行一次索引读。
    pub(crate) async fn run<T, F>(&self, read: F) -> Result<T, (String, String)>
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

/// `escode-task` channel。`data_base_dir` 由 Main 经 `rpcListen` 提交；缺省时显式缺口。
pub fn task_channel(data_base_dir: Option<PathBuf>) -> Channel {
    let index = Arc::new(Mutex::new(LazyIndex::default()));
    let Some(data_base_dir) = data_base_dir else {
        let gap = "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.";
        let missing = || {
            handler(move |_arg: Wire| {
                let gap = gap.to_owned();
                Box::pin(async move { Err(("InvalidRequest".to_owned(), gap)) })
            })
        };
        return Channel::new(
            "escode-task",
            vec![
                ("getTaskMeta", missing()),
                ("listTasks", missing()),
                ("listPinnedTaskIds", missing()),
                ("listPinnedTasks", missing()),
                ("listDeletedTaskIds", missing()),
                ("listArchivedTasks", missing()),
                ("listTaskList", missing()),
                ("listGroupedTaskViewStructure", missing()),
            ],
        );
    };
    let path = tasks_index_path(&data_base_dir);

    let get_task_meta = handler({
        let context = ReadContext {
            path: path.clone(),
            index: Arc::clone(&index),
        };
        move |arg: Wire| {
            let params = first_object(arg);
            let context = context_ref(&context);
            Box::pin(async move {
                let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
                if task_id.is_empty() {
                    return Err((
                        "InvalidRequest".to_owned(),
                        "taskId is required.".to_owned(),
                    ));
                }
                let workspace_path = workspace_path_of(&params);
                let identity = workspace_identity_of(&params);
                context
                    .run(move |index| {
                        index
                            .get_task_meta(&workspace_path, identity.as_deref(), &task_id)
                            .map(|meta| meta.map_or(Wire::Undefined, Wire::Object))
                            .map_err(|error| error.to_string())
                    })
                    .await
            })
        }
    });

    // 列表分区：同一份 SQL，只是过滤组合不同（与 TS 三个服务方法一一对应）。
    let partition = |provider: bool, pinned: Option<bool>, archived: Option<bool>| {
        let context = ReadContext {
            path: path.clone(),
            index: Arc::clone(&index),
        };
        handler(move |arg: Wire| {
            let params = first_object(arg);
            let context = context_ref(&context);
            Box::pin(async move {
                let workspace_path = workspace_path_of(&params);
                let identity = workspace_identity_of(&params);
                context
                    .run(move |index| {
                        let filters = TaskFilters {
                            workspace_path: if workspace_path.is_empty() {
                                None
                            } else {
                                Some(workspace_path.as_str())
                            },
                            workspace_identity: identity.as_deref(),
                            provider: provider.then_some(GLM_PROVIDER),
                            pinned,
                            archived,
                            include_deleted: false,
                        };
                        index
                            .list_task_metas(&filters)
                            .map(|metas| Wire::Array(metas.into_iter().map(Wire::Object).collect()))
                            .map_err(|error| error.to_string())
                    })
                    .await
            })
        })
    };

    let list_deleted_task_ids = handler({
        let context = ReadContext {
            path: path.clone(),
            index: Arc::clone(&index),
        };
        move |arg: Wire| {
            let params = first_object(arg);
            let context = context_ref(&context);
            Box::pin(async move {
                let workspace_path = workspace_path_of(&params);
                if workspace_path.is_empty() {
                    return Err((
                        "InvalidRequest".to_owned(),
                        "workspacePath is required.".to_owned(),
                    ));
                }
                let identity = workspace_identity_of(&params);
                context
                    .run(move |index| {
                        index
                            .list_deleted_task_ids(
                                &workspace_path,
                                identity.as_deref(),
                                Some(GLM_PROVIDER),
                            )
                            .map(|ids| {
                                Wire::Array(ids.into_iter().map(Wire::Str).collect::<Vec<_>>())
                            })
                            .map_err(|error| error.to_string())
                    })
                    .await
            })
        }
    });

    // `listPinnedTaskIds` 返回的是 id 数组（TS 同名方法的返回类型），不是 meta。
    let list_pinned_task_ids = {
        let context = ReadContext {
            path: path.clone(),
            index: Arc::clone(&index),
        };
        handler(move |_arg: Wire| {
            let context = context_ref(&context);
            Box::pin(async move {
                context
                    .run(move |index| {
                        let filters = TaskFilters {
                            provider: Some(GLM_PROVIDER),
                            pinned: Some(true),
                            archived: Some(false),
                            ..TaskFilters::default()
                        };
                        index
                            .list_task_metas(&filters)
                            .map(|metas| {
                                Wire::Array(
                                    metas
                                        .iter()
                                        .map(|meta| {
                                            Wire::Str(
                                                meta["taskId"]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .to_owned(),
                                            )
                                        })
                                        .collect(),
                                )
                            })
                            .map_err(|error| error.to_string())
                    })
                    .await
            })
        })
    };

    let list_task_list = domains_task_query::list_task_list_handler(ReadContext {
        path: path.clone(),
        index: Arc::clone(&index),
    });
    let list_grouped_structure = domains_task_query::list_grouped_structure_handler(ReadContext {
        path: path.clone(),
        index: Arc::clone(&index),
    });

    let mut methods: Vec<(&'static str, crate::session::MethodHandler)> = vec![
        ("getTaskMeta", get_task_meta),
        ("listTasks", partition(true, Some(false), Some(false))),
        ("listPinnedTaskIds", list_pinned_task_ids),
        ("listPinnedTasks", partition(true, Some(true), Some(false))),
        // TS `listArchivedTasks` 只过滤 archived，不过滤 pinned。
        ("listArchivedTasks", partition(true, None, Some(true))),
        ("listDeletedTaskIds", list_deleted_task_ids),
        ("listTaskList", list_task_list),
        ("listGroupedTaskViewStructure", list_grouped_structure),
    ];
    methods.shrink_to_fit();
    Channel::new("escode-task", methods)
}

/// 参数取值：`workspacePath` 缺省为空串（`listPinnedTaskIds` 不传）。
fn workspace_path_of(params: &Value) -> String {
    params["workspacePath"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// `workspaceIdentity` 空白视同未提供（AGENTS.md Workspace Identity）。
fn workspace_identity_of(params: &Value) -> Option<String> {
    params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// RPC 实参：`ProxyChannel.toService` 把 JS 实参打包成数组，取第一个并当对象用。
pub(crate) fn first_object(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

/// 借出上下文（闭包捕获需要 `'static`，这里只取字段的引用副本）。
pub(crate) fn context_ref(context: &ReadContext) -> ReadContext {
    ReadContext {
        path: context.path.clone(),
        index: Arc::clone(&context.index),
    }
}

#[cfg(test)]
#[path = "domains_task_tests.rs"]
mod tests;
