//! `escode-task` 批量归档（TS `archiveStaleTasks` / `archiveWorkspaceTasks`）。
//!
//! 与单条 `archiveTask` 共用 `workspace_task_list_changed` 事件与索引写者；从 `domains_task_state`
//! 拆出以控制单文件行数。

use escode_host_services::tasks_index::StaleArchiveParams;

use crate::domains_task_state::{StateContext, index_error, method, with_index};
use crate::session::{MethodHandler, RpcError};
use crate::wire::Wire;
use crate::workspace_events::Target;

pub(super) fn archive_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("archiveStaleTasks", archive_stale(context)),
        ("archiveWorkspaceTasks", archive_workspace(context)),
    ]
}

/// TS `archiveStaleTasks`：按天数批量归档，逐个任务广播 `task_meta_changed`。
fn archive_stale(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let workspace_path = params["workspacePath"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if workspace_path.is_empty() {
            return Err(RpcError::new(
                "InvalidRequest",
                "workspacePath is required.",
            ));
        }
        let identity = params["workspaceIdentity"].as_str().map(str::to_owned);
        // TS `Math.max(1, Math.floor(olderThanDays))`。
        let days = params["olderThanDays"]
            .as_f64()
            .unwrap_or(0.0)
            .floor()
            .max(1.0) as i64;
        let provider = params["provider"].as_str().map(str::to_owned);
        let (path, index_identity, index_provider) =
            (workspace_path.clone(), identity.clone(), provider);
        let metas = with_index(&context, move |index| {
            index.archive_stale_tasks(&StaleArchiveParams {
                workspace_path: &path,
                workspace_identity: index_identity.as_deref(),
                older_than_days: days,
                provider: index_provider.as_deref(),
            })
        })
        .await?
        .map_err(|error| index_error(&error))?;
        for meta in &metas {
            context.events.task_list_changed(
                &Target {
                    workspace_path: &workspace_path,
                    workspace_identity: identity.as_deref(),
                    task_id: meta["taskId"].as_str(),
                },
                Some(meta),
                "task_meta_changed",
                None,
            );
        }
        Ok(Wire::Array(metas.into_iter().map(Wire::Object).collect()))
    })
}

/// TS `archiveWorkspaceTasks`：workspace 下全部未归档任务一次归档，广播一次 workspace 级事件。
fn archive_workspace(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let workspace_path = params["workspacePath"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if workspace_path.is_empty() {
            return Err(RpcError::new(
                "InvalidRequest",
                "workspacePath is required.",
            ));
        }
        let identity = params["workspaceIdentity"].as_str().map(str::to_owned);
        let (path, index_identity) = (workspace_path.clone(), identity.clone());
        let archived = with_index(&context, move |index| {
            index.archive_workspace_tasks(&path, index_identity.as_deref())
        })
        .await?
        .map_err(|error| index_error(&error))?;
        context.events.task_list_changed(
            &Target {
                workspace_path: &workspace_path,
                workspace_identity: identity.as_deref(),
                task_id: None,
            },
            None,
            "task_meta_changed",
            None,
        );
        Ok(Wire::Array(
            archived.into_iter().map(Wire::Object).collect(),
        ))
    })
}
