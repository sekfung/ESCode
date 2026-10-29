//! TS `repairSubagentTaskIndex` 的 Host 侧移植：sessions-index 首帧播种后清扫旧索引里
//! 已存在的 `subagent_child` 行（旧版本曾把冷恢复 child 写成主列表行）。
//!
//! 状态所有权：本模块只经 `ReadContext`（单写者）软删除派生索引行，不写 CLI 转录、
//! 不改 schema；行身份必须由 CLI `session/list`（existing-only）权威确认。

use std::sync::Arc;

use escode_host_control::logging;
use escode_host_services::tasks_index::TaskFilters;
use escode_host_services::tasks_index::TasksIndexError;
use escode_host_services::tasks_index::write_state::{StatePatch, TaskRef};
use serde_json::{Value, json};

use super::Ingest;
use super::apply_writes::AGENT_PROVIDER;
use crate::forward::RuntimePolicy;
use crate::workspace_events::Target;

/// TS `repairSubagentTaskIndex` 的 `IDENTITY_QUERY_BATCH_SIZE`（`session/list` 的 sessionIds 上限）。
const IDENTITY_QUERY_BATCH_SIZE: usize = 64;

/// 首帧播种后清扫**存量**行：候选 = 本 workspace 索引行差集于首帧可见 sessionId；只有 CLI
/// `session/list`（explicit ids、existing-only，不拉起 runtime）明确回 `subagent_child` 且
/// workspace 身份匹配的行才软删除。消失的派生行不删 CLI 转录；查询失败 fail-closed（只 warn）。
pub(super) async fn repair_subagent_rows(
    ingest: &Arc<Ingest>,
    visible: &std::collections::BTreeSet<String>,
) {
    let rows = {
        let read = ingest.read.clone();
        let workspace_path = ingest.workspace_path.clone();
        let workspace_identity = ingest.workspace_identity.clone();
        read.run(move |index| {
            index
                .list_task_metas(&TaskFilters {
                    workspace_path: Some(&workspace_path),
                    workspace_identity: workspace_identity.as_deref(),
                    provider: Some(AGENT_PROVIDER),
                    include_deleted: false,
                    ..TaskFilters::default()
                })
                .map_err(|error| error.to_string())
        })
        .await
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err((code, message)) => {
            logging::warn(
                "task-index",
                format!("subagent index repair skipped: {code}: {message}"),
            );
            return;
        }
    };
    let candidates: Vec<String> = rows
        .iter()
        .filter_map(|row| row["taskId"].as_str())
        .filter(|task_id| !visible.contains(*task_id))
        .map(str::to_owned)
        .collect();
    if candidates.is_empty() {
        return;
    }
    let workspace_ref = workspace_ref_json(ingest);
    for batch in candidates.chunks(IDENTITY_QUERY_BATCH_SIZE) {
        let body = json!({
            "sessionIds": batch,
            "includeArchived": true,
            "workspace": &workspace_ref,
        });
        let listed = ingest
            .base
            .call(
                &ingest.target_json(),
                RuntimePolicy::ExistingOnly,
                "session/list",
                body,
            )
            .await;
        let sessions = match listed {
            Ok(called) => called.result["sessions"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            Err(error) => {
                // TS 同样整轮放弃：查询失败不能推断任何行可删除。
                logging::warn(
                    "task-index",
                    format!("subagent index repair list failed: {}", error.message),
                );
                return;
            }
        };
        for session in sessions {
            if session["sessionKind"] != "subagent_child" {
                continue;
            }
            let Some(session_id) = session["sessionId"].as_str() else {
                continue;
            };
            if !batch.iter().any(|task_id| task_id == session_id) {
                continue;
            }
            if session_workspace_key(&session["workspace"]) != Some(ingest.key.as_str()) {
                continue;
            }
            if !soft_delete_row(ingest, session_id).await {
                continue;
            }
            ingest.events.task_list_changed(
                &Target {
                    workspace_path: &ingest.workspace_path,
                    workspace_identity: ingest.workspace_identity.as_deref(),
                    task_id: None,
                },
                None,
                "task_meta_changed",
                None,
            );
        }
    }
    logging::info(
        "task-index",
        format!(
            "subagent index repair finished workspace={} candidates={}",
            ingest.workspace_path,
            candidates.len()
        ),
    );
}

/// `session/list` 的 workspace ref（TS `buildWorkspaceRef` 同形）。
fn workspace_ref_json(ingest: &Arc<Ingest>) -> Value {
    let mut workspace = json!({
        "workspacePath": ingest.workspace_path.clone(),
        "workspaceKey": ingest.key.clone(),
    });
    if let Some(identity) = &ingest.workspace_identity {
        workspace["workspaceIdentity"] = Value::from(identity.clone());
    }
    workspace
}

/// 返回的 workspace 身份键：`workspaceKey ?? workspaceIdentity.trim() ?? workspacePath`。
fn session_workspace_key(workspace: &Value) -> Option<&str> {
    workspace["workspaceKey"]
        .as_str()
        .or_else(|| {
            workspace["workspaceIdentity"]
                .as_str()
                .map(str::trim)
                .filter(|identity| !identity.is_empty())
        })
        .or_else(|| workspace["workspacePath"].as_str())
}

/// 软删除一行；行已被其他路径删除时按 `TaskNotFound` 幂等跳过。返回是否真正删除。
async fn soft_delete_row(ingest: &Arc<Ingest>, session_id: &str) -> bool {
    let read = ingest.read.clone();
    let workspace_path = ingest.workspace_path.clone();
    let workspace_identity = ingest.workspace_identity.clone();
    let task_id = session_id.to_owned();
    let deleted = read
        .run(move |index| {
            match index.update_task_state(
                &TaskRef {
                    workspace_path: &workspace_path,
                    workspace_identity: workspace_identity.as_deref(),
                    task_id: &task_id,
                },
                &StatePatch {
                    deleted: Some(true),
                    ..StatePatch::default()
                },
            ) {
                Ok(_) => Ok(true),
                Err(TasksIndexError::TaskNotFound { .. }) => Ok(false),
                Err(error) => Err(error.to_string()),
            }
        })
        .await;
    match deleted {
        Ok(deleted) => deleted,
        Err((code, message)) => {
            logging::warn(
                "task-index",
                format!("subagent row delete failed taskId={session_id}: {code}: {message}"),
            );
            false
        }
    }
}
