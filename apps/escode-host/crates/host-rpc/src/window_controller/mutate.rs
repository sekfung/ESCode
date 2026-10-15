//! `window-controller.mutateTask` / `deleteArchivedTask(s)`（TS `windowHostControllerService.ts:586-645`）。
//!
//! 写操作复用 `escode-task` 的同一写面（`domains_task_state` / `domains_task_delete`）：索引仍是唯一
//! 写者，事件 reason 与直接调用 `escode-task` 时逐字相同（TS 的 controller 也是转到 source 的
//! taskService）。写完后 `force` 刷新投影并返回投影行的 meta（TS：`getTasks().find(...)?.meta ?? null`）。

use std::sync::Arc;

use escode_host_services::tasks_index::write_state::StatePatch;
use serde_json::{Value, json};

use super::WindowController;
use crate::domains_task_state::{StateContext, Task, apply_state_patch, apply_unread};
use crate::domains_task_support::first_object;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

pub(super) fn mutate_handler(controller: &Arc<WindowController>) -> MethodHandler {
    let controller = Arc::clone(controller);
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let controller = Arc::clone(&controller);
        Box::pin(async move {
            let address = params["address"].clone();
            let mutation = params["mutation"].clone();
            let scope = execute(&controller, &address, &mutation).await?;
            controller
                .refresh(&scope, true)
                .await
                .map_err(refresh_error)?;
            let task_id = address["taskId"].as_str().unwrap_or_default();
            let meta = controller
                .projector()
                .row(&address, task_id)
                .map(|row| row["meta"].clone())
                .unwrap_or(Value::Null);
            Ok(Wire::Object(meta))
        })
    })
}

pub(super) fn delete_archived_task_handler(controller: &Arc<WindowController>) -> MethodHandler {
    let controller = Arc::clone(controller);
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let controller = Arc::clone(&controller);
        Box::pin(async move {
            let address = params["address"].clone();
            // TS：条件删除允许目标已被其它客户端删除（行缺失不报错）。
            let scope = controller.projector().validate_mutation(&address, true)?;
            let task = task_of(&address);
            let deleted = super::super::domains_task_delete::delete_archived_one(
                &controller.state_context(),
                &task,
            )
            .await?;
            // 删除已经持久成功：列表重查失败不能改报删除失败（TS `.catch(onSourceError)`）。
            if let Err(error) = controller.refresh(&scope, true).await {
                eprintln!(
                    "escode-host: window-controller refresh after archived delete failed: {}",
                    error.message
                );
            }
            Ok(Wire::Object(Value::Bool(deleted)))
        })
    })
}

pub(super) fn delete_archived_tasks_handler(controller: &Arc<WindowController>) -> MethodHandler {
    let controller = Arc::clone(controller);
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let controller = Arc::clone(&controller);
        Box::pin(async move {
            let address = params["address"].clone();
            let task_ids: Vec<String> = params["taskIds"]
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            if task_ids.is_empty() {
                return Ok(Wire::Object(json!({
                    "deletedTaskIds": [],
                    "skippedTaskIds": [],
                    "failedTaskIds": [],
                })));
            }
            let scope = controller.projector().validate_mutation(&address, true)?;
            let workspace_path = address["workspacePath"].as_str().unwrap_or_default();
            let identity = address["workspaceIdentity"].as_str();
            let result = super::super::domains_task_delete::delete_archived_batch(
                &controller.state_context(),
                workspace_path,
                identity,
                &task_ids,
            )
            .await?;
            if let Err(error) = controller.refresh(&scope, true).await {
                eprintln!(
                    "escode-host: window-controller refresh after archived batch delete failed: {}",
                    error.message
                );
            }
            Ok(Wire::Object(result))
        })
    })
}

/// TS `executeMutation`：按 kind 映射到 source 的写面；`open`/`resume` 只做校验（无写）。
async fn execute(
    controller: &Arc<WindowController>,
    address: &Value,
    mutation: &Value,
) -> Result<super::rows::LocalScope, RpcError> {
    let kind = mutation["kind"].as_str().unwrap_or_default();
    // 归档删除允许行缺失（条件删除的幂等跳过）；其余 kind 必须有投影行。
    let allow_missing = matches!(kind, "delete-archived" | "delete-archived-batch");
    let scope = controller
        .projector()
        .validate_mutation(address, allow_missing)?;
    let state = controller.state_context();
    let task = task_of(address);
    match kind {
        "pin" => {
            let pinned = mutation["pinned"] == Value::Bool(true);
            let reason = if pinned {
                "task_pinned"
            } else {
                "task_unpinned"
            };
            write_patch(&state, &task, pinned_patch(pinned), reason).await?;
        }
        "archive" => {
            let archived = mutation["archived"] == Value::Bool(true);
            let reason = if archived {
                "task_archived"
            } else {
                "task_unarchived"
            };
            write_patch(&state, &task, archived_patch(archived), reason).await?;
        }
        "delete" => {
            write_patch(
                &state,
                &task,
                StatePatch {
                    deleted: Some(true),
                    ..StatePatch::default()
                },
                "task_deleted",
            )
            .await?;
        }
        "delete-archived" => {
            super::super::domains_task_delete::delete_archived_one(&state, &task).await?;
        }
        "delete-archived-batch" => {
            let task_ids: Vec<String> = mutation["taskIds"]
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let workspace_path = address["workspacePath"].as_str().unwrap_or_default();
            let identity = address["workspaceIdentity"].as_str();
            super::super::domains_task_delete::delete_archived_batch(
                &state,
                workspace_path,
                identity,
                &task_ids,
            )
            .await?;
        }
        "mark-read" => {
            apply_unread(&state, &task, false, mutation["expectedUnreadAt"].as_i64()).await?;
        }
        "mark-unread" => {
            apply_unread(&state, &task, true, None).await?;
        }
        // TS：open/resume 在 controller 层是 no-op（只校验唯一 source；会话 IO 走 attachment facade）。
        "open" | "resume" => {}
        other => {
            return Err(RpcError::new(
                "InvalidRequest",
                format!("Unknown controller mutation `{other}`"),
            ));
        }
    }
    Ok(scope)
}

async fn write_patch(
    state: &StateContext,
    task: &Task,
    patch: StatePatch,
    reason: &str,
) -> Result<(), RpcError> {
    apply_state_patch(state, task, patch, reason)
        .await
        .map(|_| ())
}

fn pinned_patch(pinned: bool) -> StatePatch {
    StatePatch {
        pinned: Some(pinned),
        ..StatePatch::default()
    }
}

fn archived_patch(archived: bool) -> StatePatch {
    StatePatch {
        archived: Some(archived),
        ..StatePatch::default()
    }
}

/// 控制器地址 → 写面任务定位（`remoteSessionId` 不参与：本片只做本地 source）。
pub(super) fn task_of(address: &Value) -> Task {
    Task {
        workspace_path: address["workspacePath"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        workspace_identity: address["workspaceIdentity"].as_str().map(str::to_owned),
        task_id: address["taskId"].as_str().unwrap_or_default().to_owned(),
    }
}

fn refresh_error(error: RpcError) -> RpcError {
    RpcError::new(error.name, error.message)
}
