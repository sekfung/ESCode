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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use escode_host_services::tasks_index::TaskFilters;
use serde_json::Value;

use super::domains_task_query;
use crate::domains_task_support::{
    GLM_PROVIDER, LazyIndex, ReadContext, RuntimeAccess, context_ref, first_object,
    tasks_index_path, workspace_identity_of, workspace_path_of,
};
use crate::session::{Channel, handler};
use crate::task_targets::{TaskTargets, remember_from_meta};
use crate::wire::Wire;

/// `escode-task` channel。`data_base_dir` 由 Main 经 `rpcListen` 提交；缺省时显式缺口。
/// `runtime` 用于 `getTaskModelSelection` 这类需要读运行时会话状态的方法（TS 同样读运行时）。
pub fn task_channel(data_base_dir: Option<PathBuf>, runtime: Option<RuntimeAccess>) -> Channel {
    let index = Arc::new(Mutex::new(LazyIndex::default()));
    let targets = Arc::new(TaskTargets::new());
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
                ("getTaskModelSelection", missing()),
                ("getTaskSnapshot", missing()),
                ("getTaskSnapshotWithEtag", missing()),
                ("getTaskSnapshotBody", missing()),
                ("getTaskSnapshotRef", missing()),
                ("getTaskSnapshotToolCallsSlice", missing()),
                ("getTaskConfigOptions", missing()),
            ],
        );
    };
    let path = tasks_index_path(&data_base_dir);

    let get_task_meta = handler({
        let context = ReadContext {
            path: path.clone(),
            index: Arc::clone(&index),
            targets: Arc::clone(&targets),
            runtime: runtime.clone(),
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
    // 闭包只借用下面的两个句柄副本（move 闭包不能直接借用外层的 Arc）。
    let targets_for_partition = Arc::clone(&targets);
    let runtime_for_partition = runtime.clone();
    let path_for_partition = path.clone();
    let index_for_partition = Arc::clone(&index);
    let partition = move |provider: bool, pinned: Option<bool>, archived: Option<bool>| {
        let targets = Arc::clone(&targets_for_partition);
        let runtime = runtime_for_partition.clone();
        let context = ReadContext {
            path: path_for_partition.clone(),
            index: Arc::clone(&index_for_partition),
            targets: Arc::clone(&targets),
            runtime,
        };
        handler(move |arg: Wire| {
            let params = first_object(arg);
            let context = context_ref(&context);
            // 每次调用克隆一次的 Arc（闭包是 Fn：不能把捕获的 Arc move 出去）。
            let targets_for_read = Arc::clone(&targets);
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
                            .map(|metas| {
                                for meta in &metas {
                                    remember_from_meta(&targets_for_read, meta);
                                }
                                Wire::Array(metas.into_iter().map(Wire::Object).collect())
                            })
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
            targets: Arc::clone(&targets),
            runtime: runtime.clone(),
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
            targets: Arc::clone(&targets),
            runtime: runtime.clone(),
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
        targets: Arc::clone(&targets),
        runtime: runtime.clone(),
    });
    let list_grouped_structure = domains_task_query::list_grouped_structure_handler(ReadContext {
        path: path.clone(),
        index: Arc::clone(&index),
        targets: Arc::clone(&targets),
        runtime: runtime.clone(),
    });
    // 快照读面要各自独立的 ReadContext（handler 是 Fn，句柄必须克隆进去）。
    let path_for_snapshot = path;
    let index_for_snapshot = Arc::clone(&index);
    let targets_for_snapshot = Arc::clone(&targets);
    let runtime_for_snapshot = runtime.clone();

    // `getTaskModelSelection`：只有 `{taskId}` 入参 —— 注册表解析目标后读运行时会话的当前模型。
    let get_task_model_selection = handler(move |arg: Wire| {
        let params = first_object(arg);
        let targets = Arc::clone(&targets);
        let runtime = runtime.clone();
        Box::pin(async move {
            let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
            if task_id.is_empty() {
                return Err((
                    "InvalidRequest".to_owned(),
                    "taskId is required.".to_owned(),
                ));
            }
            let target = targets.get(&task_id)?;
            let Some(runtime) = runtime.as_ref() else {
                return Err((
                    "NotImplemented".to_owned(),
                    "escode-task runtime access is not configured for this Host".to_owned(),
                ));
            };
            let mut request = serde_json::json!({
                "workspacePath": target.workspace_path,
                "sessionId": task_id,
            });
            if let Some(identity) = target.workspace_identity.as_ref() {
                request["workspaceIdentity"] = Value::String(identity.clone());
            }
            // TS `getTaskModelSelection`：直接读运行时 `session/read` 的 `settings.model.current`。
            let answer = crate::forward::runtime_call(
                &runtime.instances,
                &runtime.options,
                "session/read",
                request,
                crate::forward::Extract::Result,
            )
            .await?;
            // `Extract::Result` 已把 `result` 解包成对象；服务面只回 `settings.model.current`。
            let Wire::Object(answer) = answer else {
                return Err((
                    "Fatal".to_owned(),
                    "session/read returned a non-object result".to_owned(),
                ));
            };
            let current = answer
                .get("settings")
                .and_then(|settings| settings.get("model"))
                .and_then(|model| model.get("current"))
                .cloned()
                .unwrap_or(Value::Null);
            Ok(if current.is_null() {
                Wire::Undefined
            } else {
                Wire::Object(current)
            })
        })
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
        ("getTaskModelSelection", get_task_model_selection),
        // S2b 快照读面：resume + 投影 + 索引回写（见 domains_task_snapshot.rs）。
        (
            "getTaskSnapshot",
            super::domains_task_snapshot::snapshot_handler(
                ReadContext {
                    path: path_for_snapshot.clone(),
                    index: Arc::clone(&index_for_snapshot),
                    targets: Arc::clone(&targets_for_snapshot),
                    runtime: runtime_for_snapshot.clone(),
                },
                false,
            ),
        ),
        (
            "getTaskSnapshotWithEtag",
            super::domains_task_snapshot::snapshot_handler(
                ReadContext {
                    path: path_for_snapshot.clone(),
                    index: Arc::clone(&index_for_snapshot),
                    targets: Arc::clone(&targets_for_snapshot),
                    runtime: runtime_for_snapshot.clone(),
                },
                true,
            ),
        ),
        (
            "getTaskSnapshotBody",
            super::domains_task_snapshot::null_handler(),
        ),
        (
            "getTaskSnapshotRef",
            super::domains_task_snapshot::null_handler(),
        ),
        (
            "getTaskSnapshotToolCallsSlice",
            super::domains_task_snapshot::null_handler(),
        ),
        (
            "getTaskConfigOptions",
            super::domains_task_snapshot::config_options_handler(ReadContext {
                path: path_for_snapshot,
                index: Arc::clone(&index_for_snapshot),
                targets: Arc::clone(&targets_for_snapshot),
                runtime: runtime_for_snapshot,
            }),
        ),
    ];
    methods.shrink_to_fit();
    Channel::new("escode-task", methods)
}

#[cfg(test)]
#[path = "domains_task_tests.rs"]
mod tests;
