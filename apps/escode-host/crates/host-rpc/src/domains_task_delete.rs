//! `escode-task` 删除类状态写（TS `deleteArchivedTask` / `deleteArchivedTasks`），从
//! `domains_task_state.rs` 拆出以守住单文件行数上限。

use escode_host_services::tasks_index::write_state::TaskRef;
use serde_json::{Value, json};

use super::domains_task_state::{StateContext, Task, index_error, method, with_index};
use crate::session::{MethodHandler, RpcError};
use crate::wire::Wire;
use crate::workspace_events::Target;

async fn delete_one(context: &StateContext, task: &Task) -> Result<Option<Value>, RpcError> {
    let (path, identity, id) = (
        task.workspace_path.clone(),
        task.workspace_identity.clone(),
        task.task_id.clone(),
    );
    with_index(context, move |index| {
        index.delete_archived_task(&TaskRef {
            workspace_path: &path,
            workspace_identity: identity.as_deref(),
            task_id: &id,
        })
    })
    .await?
    .map_err(|error| index_error(&error))
}

/// TS `deleteArchivedTask`：未归档/已删除 → false。
pub(super) fn delete_archived_task(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let task = Task::parse(&params)?;
        let Some(meta) = delete_one(&context, &task).await? else {
            return Ok(Wire::Object(Value::Bool(false)));
        };
        context
            .events
            .task_list_changed(&task.target(), Some(&meta), "task_deleted", None);
        Ok(Wire::Object(Value::Bool(true)))
    })
}

/// TS `deleteArchivedTasks`：逐项事务（一项失败不回滚其它）、去重保序、批次只发一次事件。
pub(super) fn delete_archived_tasks(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let workspace_path = params["workspacePath"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let workspace_identity = params["workspaceIdentity"].as_str().map(str::to_owned);
        let mut seen = std::collections::BTreeSet::new();
        let task_ids: Vec<String> = params["taskIds"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| seen.insert((*id).to_owned()))
            .map(str::to_owned)
            .collect();
        let (mut deleted, mut skipped, mut failed) = (Vec::new(), Vec::new(), Vec::new());
        for task_id in task_ids {
            let task = Task {
                workspace_path: workspace_path.clone(),
                workspace_identity: workspace_identity.clone(),
                task_id: task_id.clone(),
            };
            match delete_one(&context, &task).await {
                Ok(Some(_)) => deleted.push(task_id),
                Ok(None) => skipped.push(task_id),
                Err(error) => {
                    eprintln!(
                        "escode-host: [ArchivedTaskDeletion] {task_id} failed: {}",
                        error.message
                    );
                    failed.push(task_id);
                }
            }
        }
        if !deleted.is_empty() {
            context.events.task_list_changed(
                &Target {
                    workspace_path: &workspace_path,
                    workspace_identity: workspace_identity.as_deref(),
                    task_id: None,
                },
                None,
                "task_deleted",
                None,
            );
        }
        Ok(Wire::Object(json!({
            "deletedTaskIds": deleted,
            "skippedTaskIds": skipped,
            "failedTaskIds": failed,
        })))
    })
}
