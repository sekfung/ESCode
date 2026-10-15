//! `escode-task` 的侧栏状态写（S3a，TS `escodeTaskServiceAdapter.ts:3275-3460`）：
//! pin / unread / archive / rename / delete，以及写后的 `workspace_task_list_changed` 事件。
//!
//! 每个方法的 `reason` 与 TS 逐字一致（Renderer 按它决定局部合并还是 membership 重拉）：
//! `task_pinned`/`task_unpinned`、`task_meta_changed`（unread）、`task_archived`/`task_unarchived`、
//! `task_title_changed`（rename）、`task_deleted`（delete / 删归档）。
//!
//! 登记差异（docs/specs/rust-u2-tasks-index.md §6.6）：TS 另维护进程内 overlay（`setOverlay`）给
//! 并发读做乐观覆盖；Rust 在返回前已同步写库，索引即唯一事实，不另设 overlay。

use std::sync::Arc;

use escode_host_services::tasks_index::TasksIndexError;
use escode_host_services::tasks_index::write_state::{Field, StatePatch, TaskRef};
use serde_json::{Value, json};

use crate::domains_task_support::{ReadContext, first_object};
use crate::forward::{self, RuntimePolicy};
use crate::host_command::{self, HostCommand};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;
use crate::workspace_events::{Target, WorkspaceEvents};

#[derive(Clone)]
pub(super) struct StateContext {
    pub read: ReadContext,
    pub events: Arc<WorkspaceEvents>,
}

/// 一次调用的任务定位（`workspacePath` + `workspaceIdentity?` + `taskId`）。
pub(super) struct Task {
    pub(super) workspace_path: String,
    pub(super) workspace_identity: Option<String>,
    pub(super) task_id: String,
}

impl Task {
    pub(super) fn parse(params: &Value) -> Result<Self, RpcError> {
        let workspace_path = params["workspacePath"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
        if workspace_path.is_empty() || task_id.is_empty() {
            return Err(RpcError::new(
                "InvalidRequest",
                "workspacePath and taskId are required.",
            ));
        }
        Ok(Self {
            workspace_path,
            // TS 原样透传 `params.workspaceIdentity`（事件与目标都带原值）。
            workspace_identity: params["workspaceIdentity"].as_str().map(str::to_owned),
            task_id,
        })
    }

    pub(super) fn target(&self) -> Target<'_> {
        Target {
            workspace_path: &self.workspace_path,
            workspace_identity: self.workspace_identity.as_deref(),
            task_id: Some(&self.task_id),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(super) fn index_error(error: &TasksIndexError) -> RpcError {
    match error {
        // TS 抛普通 Error（文案相同）。
        TasksIndexError::TaskNotFound { .. } => RpcError::new("Error", error.to_string()),
        other => RpcError::new("TasksIndexUnavailable", other.to_string()),
    }
}

/// 在索引线程里跑一次写，区分「任务不存在」与其他错误。
pub(super) async fn with_index<T: Send + 'static>(
    context: &StateContext,
    write: impl FnOnce(&escode_host_services::tasks_index::TasksIndex) -> Result<T, TasksIndexError>
    + Send
    + 'static,
) -> Result<Result<T, TasksIndexError>, RpcError> {
    let read = crate::domains_task_support::context_ref(&context.read);
    read.run(move |index| Ok(write(index)))
        .await
        .map_err(RpcError::from)
}

/// TS `updateIndexedTaskState`：缺行时按需从 runtime 播种当前任务再重试一次。
async fn update_indexed(
    context: &StateContext,
    task: &Task,
    patch: StatePatch,
) -> Result<Value, RpcError> {
    let attempt = |patch: StatePatch| {
        let (path, identity, id) = (
            task.workspace_path.clone(),
            task.workspace_identity.clone(),
            task.task_id.clone(),
        );
        with_index(context, move |index| {
            index.update_task_state(
                &TaskRef {
                    workspace_path: &path,
                    workspace_identity: identity.as_deref(),
                    task_id: &id,
                },
                &patch,
            )
        })
    };
    match attempt(patch.clone()).await? {
        Ok(meta) => Ok(meta),
        Err(TasksIndexError::TaskNotFound { .. }) => {
            eprintln!(
                "escode-host: task index row missing for {}; seeding from the agent",
                task.task_id
            );
            crate::domains_task_snapshot::seed_from_runtime(
                &context.read,
                &task.workspace_path,
                task.workspace_identity.as_deref(),
                &task.task_id,
            )
            .await
            .map_err(RpcError::from)?;
            attempt(patch).await?.map_err(|error| index_error(&error))
        }
        Err(error) => Err(index_error(&error)),
    }
}

pub(super) fn method<F, Fut>(context: &StateContext, run: F) -> MethodHandler
where
    F: Fn(StateContext, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Wire, RpcError>> + Send + 'static,
{
    let context = context.clone();
    handler_rpc(move |arg: Wire| Box::pin(run(context.clone(), first_object(arg))))
}

fn flag_method(
    context: &StateContext,
    build: fn(bool) -> StatePatch,
    key: &'static str,
    reasons: (&'static str, &'static str),
) -> MethodHandler {
    method(context, move |context, params| async move {
        let task = Task::parse(&params)?;
        let value = params[key] == true;
        let meta = update_indexed(&context, &task, build(value)).await?;
        let reason = if value { reasons.0 } else { reasons.1 };
        context
            .events
            .task_list_changed(&task.target(), Some(&meta), reason, None);
        Ok(Wire::Object(meta))
    })
}

/// 侧栏状态写的方法表（追加到 `escode-task` channel）。
pub(super) fn state_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        (
            "setTaskPinned",
            flag_method(
                context,
                |pinned| StatePatch {
                    pinned: Some(pinned),
                    ..StatePatch::default()
                },
                "pinned",
                ("task_pinned", "task_unpinned"),
            ),
        ),
        ("archiveTask", fixed_archive(context, true, "task_archived")),
        (
            "unarchiveTask",
            fixed_archive(context, false, "task_unarchived"),
        ),
        ("setTaskUnread", set_task_unread(context)),
        ("renameTask", rename_task(context)),
        ("deleteTask", delete_task(context)),
        (
            "deleteArchivedTask",
            super::domains_task_delete::delete_archived_task(context),
        ),
        (
            "deleteArchivedTasks",
            super::domains_task_delete::delete_archived_tasks(context),
        ),
    ]
}

fn fixed_archive(context: &StateContext, archived: bool, reason: &'static str) -> MethodHandler {
    method(context, move |context, params| async move {
        let task = Task::parse(&params)?;
        let patch = StatePatch {
            archived: Some(archived),
            ..StatePatch::default()
        };
        let meta = update_indexed(&context, &task, patch).await?;
        context
            .events
            .task_list_changed(&task.target(), Some(&meta), reason, None);
        Ok(Wire::Object(meta))
    })
}

/// TS `setTaskUnread`：已读且带 `expectedUnreadAt` 走 CAS（未命中不清、不广播）；否则分配/清除标记。
fn set_task_unread(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let task = Task::parse(&params)?;
        let unread = params["unread"] == true;
        if !unread && let Some(expected) = params["expectedUnreadAt"].as_i64() {
            let (path, identity, id) = (
                task.workspace_path.clone(),
                task.workspace_identity.clone(),
                task.task_id.clone(),
            );
            let (meta, cleared) = with_index(&context, move |index| {
                index.clear_task_unread_if_matches(
                    &TaskRef {
                        workspace_path: &path,
                        workspace_identity: identity.as_deref(),
                        task_id: &id,
                    },
                    expected,
                )
            })
            .await?
            .map_err(|error| index_error(&error))?;
            if cleared {
                context.events.task_list_changed(
                    &task.target(),
                    Some(&meta),
                    "task_meta_changed",
                    None,
                );
            }
            return Ok(Wire::Object(meta));
        }
        let patch = StatePatch {
            unread_at: if unread {
                Field::Set(now_ms())
            } else {
                Field::Clear
            },
            ..StatePatch::default()
        };
        let meta = update_indexed(&context, &task, patch).await?;
        // unread 归属只在 tasks-index、sessions-index 不携带：必须用 task_meta_changed 触发 membership 重拉。
        context
            .events
            .task_list_changed(&task.target(), Some(&meta), "task_meta_changed", None);
        Ok(Wire::Object(meta))
    })
}

/// TS `renameTask`：写索引（手动标题 + 更新时间）→ 尽力同步 V4 `renameSession` → `task_title_changed`。
fn rename_task(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let task = Task::parse(&params)?;
        let title = params["title"].as_str().unwrap_or_default().to_owned();
        let patch = StatePatch {
            title: Some(title.clone()),
            // 手动重命名必须推进 updatedAt：否则运行中前端 optimistic merge 会把同 updatedAt 的
            // 旧长标题当成更强 meta，标题要等任务完成才刷新（TS 注释同因）。
            updated_at: Some(now_ms()),
            title_overridden: Some(true),
            ..StatePatch::default()
        };
        let meta = update_indexed(&context, &task, patch).await?;
        if let Err(error) = sync_rename(&context, &task, &title).await {
            // 历史/导入类任务可能没有活跃 v4 session：同步失败保留 task-index 标题（TS 同）。
            eprintln!(
                "escode-host: syncing renameSession to the v4 session store failed for {}: {}",
                task.task_id, error.message
            );
        }
        context
            .events
            .task_list_changed(&task.target(), Some(&meta), "task_title_changed", None);
        Ok(Wire::Object(meta))
    })
}

async fn sync_rename(context: &StateContext, task: &Task, title: &str) -> Result<(), RpcError> {
    let Some(runtime) = context.read.runtime.as_ref() else {
        return Err(RpcError::new("NotImplemented", "no runtime access"));
    };
    let envelope = host_command::envelope(HostCommand::new(
        "renameSession",
        Some(&task.task_id),
        json!({"title": title}),
    ))?;
    let mut routing = json!({"workspacePath": task.workspace_path});
    if let Some(identity) = &task.workspace_identity {
        routing["workspaceIdentity"] = Value::from(identity.clone());
    }
    let target = forward::runtime_target(routing)?;
    let instance = forward::runtime_instance(
        &runtime.instances,
        &runtime.options,
        &target,
        RuntimePolicy::StartIfNeeded,
    )
    .await?;
    let ack = forward::request(&instance, "v4/command", envelope, None).await?;
    host_command::assert_ack_ok("renameSession", &ack, &format!("session={}", task.task_id))
}

/// TS `deleteTask`：tombstone（与分组引用同事务）→ `task_deleted` 携带 meta → `Promise<void>`。
fn delete_task(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let task = Task::parse(&params)?;
        let patch = StatePatch {
            deleted: Some(true),
            ..StatePatch::default()
        };
        let meta = update_indexed(&context, &task, patch).await?;
        context
            .events
            .task_list_changed(&task.target(), Some(&meta), "task_deleted", None);
        Ok(Wire::Undefined)
    })
}

/// `escode-task.onDynamicWorkspaceEvent(workspace)`：只登记监听（不启动 runtime）。
pub(super) fn workspace_event(events: &Arc<WorkspaceEvents>) -> crate::session::EventHandler {
    let events = Arc::clone(events);
    Arc::new(move |arg: Wire| {
        let input = arg
            .into_args()
            .into_iter()
            .next()
            .map_or(Value::Null, |value| match value {
                Wire::Str(key) => Value::from(key),
                other => other.into_object().unwrap_or(Value::Null),
            });
        Ok(events.listen(&input))
    })
}
