//! 索引写与回源收敛（TS syncer 的 `applyTerminalTransition` / `applyTitleChange` /
//! `resyncTaskIndexRowFromAgent` / `seedMissingRowsFromInitialSnapshot` /
//! `syncSnapshotAndBroadcast` 的索引部分）。

use escode_host_control::logging;
use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_services::task_projection::{
    IndexOverrides, has_user_visible_content, index_meta_from_snapshot,
    searchable_text_from_snapshot,
};
use escode_host_services::tasks_index::SyncOptions;
use escode_host_services::tasks_index::write_state::{AgentPatch, Field, TaskRef};

use crate::forward::RuntimePolicy;
use crate::task_targets::remember_from_meta;
use crate::workspace_events::Target;

use super::{Ingest, now_ms};

/// TS `INITIAL_BASELINE_SEED_BATCH_SIZE`。
const SEED_BATCH_SIZE: usize = 64;
/// TS `ESCODE_AGENT_PROVIDER`（`escode-agent-policy.ts`）。
const AGENT_PROVIDER: &str = "glm";
use super::apply::resolve_terminal_unread_signal;

/// TS `applyTerminalTransition`：终态补丁 + 广播 + 回源收敛。
pub(super) async fn apply_terminal_transition(
    ingest: &Arc<Ingest>,
    session_id: &str,
    summary: &Value,
    move_to_top: bool,
) {
    let failed = summary["phase"] == "error";
    let unread_signal = resolve_terminal_unread_signal(summary);
    // error 的 lastError 详情不在 sessions-index 摘要里（patch 不带键 = 保留现值）；
    // completed 沿旧语义清空。
    let patch = AgentPatch {
        status: Some(if failed { "error" } else { "completed" }.to_owned()),
        updated_at: Some(i64::try_from(now_ms()).unwrap_or(0)),
        last_error: if failed { Field::Keep } else { Field::Clear },
        ..AgentPatch::default()
    };
    match patch_index(ingest, session_id, patch).await {
        Ok(Some(meta)) => {
            ingest.events.task_list_changed(
                &target_of(ingest, &meta, session_id),
                Some(&meta),
                "task_status_changed",
                unread_signal,
            );
            resync_row(ingest, session_id, move_to_top).await;
        }
        Ok(None) => {
            // 行缺失：把同一 signal 交给回源结果，保证提醒既不丢也不重复。
            resync_row_with_unread(ingest, session_id, move_to_top, unread_signal).await;
        }
        Err((code, message)) => {
            logging::warn(
                "task-index",
                format!("terminal patch failed taskId={session_id}: {code}: {message}"),
            );
        }
    }
}

/// TS `applyTitleChange`。
pub(super) async fn apply_title_change(ingest: &Arc<Ingest>, session_id: &str, title: &str) {
    let patch = AgentPatch {
        title: Some(title.to_owned()),
        updated_at: Some(i64::try_from(now_ms()).unwrap_or(0)),
        ..AgentPatch::default()
    };
    match patch_index(ingest, session_id, patch).await {
        Ok(Some(meta)) => {
            // 标题变更是专属 reason（不触发全局 membership 重拉）。
            ingest.events.task_list_changed(
                &target_of(ingest, &meta, session_id),
                Some(&meta),
                "task_title_changed",
                None,
            );
        }
        Ok(None) => {
            // draft 不预写占位行：首个标题先于 snapshot 到达时按完整快照回源建行。
            resync_row(ingest, session_id, true).await;
        }
        Err((code, message)) => {
            logging::warn(
                "task-index",
                format!("title patch failed taskId={session_id}: {code}: {message}"),
            );
        }
    }
}

/// TS `resyncTaskIndexRowFromAgent`：`readSession(existing-only, contentProfile:"index")` →
/// 索引收敛 + 广播（回源是状态/正文同步，reason 恒为 task_status_changed）。
pub(super) async fn resync_row(ingest: &Arc<Ingest>, session_id: &str, move_to_top: bool) {
    resync_row_with_unread(ingest, session_id, move_to_top, None).await;
}

async fn resync_row_with_unread(
    ingest: &Arc<Ingest>,
    session_id: &str,
    move_to_top: bool,
    unread_signal: Option<&str>,
) {
    let body = json!({"sessionId": session_id, "contentProfile": "index"});
    let snapshot = match ingest
        .base
        .call(
            &ingest.target_json(),
            RuntimePolicy::ExistingOnly,
            "session/read",
            body,
        )
        .await
    {
        Ok(called) => called.result,
        Err(error) => {
            logging::warn(
                "task-index",
                format!("row resync failed taskId={session_id}: {}", error.message),
            );
            return;
        }
    };
    sync_snapshot_to_index(
        ingest,
        &snapshot,
        "task_status_changed",
        unread_signal,
        move_to_top,
    )
    .await;
}

/// TS `seedMissingRowsFromInitialSnapshot`：固定小批次 insert-if-missing，失败只汇总日志。
pub(super) async fn seed_missing_rows(ingest: &Arc<Ingest>) {
    let candidates: Vec<Value> = {
        let core = super::lock(&ingest.core);
        core.summaries
            .values()
            .filter(|summary| summary["phase"] != "draft")
            .cloned()
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let mut failed_count = 0;
    for batch in candidates.chunks(SEED_BATCH_SIZE) {
        for summary in batch {
            let meta = baseline_meta(ingest, summary);
            let read = ingest.read.clone();
            let seeded = read
                .run(move |index| {
                    index
                        .seed_task_meta_if_missing(
                            meta.as_object().expect("baseline meta is an object"),
                        )
                        .map_err(|error| error.to_string())
                })
                .await;
            if let Err((code, message)) = seeded {
                failed_count += 1;
                logging::warn(
                    "task-index",
                    format!("baseline seed failed: {code}: {message}"),
                );
            }
        }
    }
    if failed_count > 0 {
        logging::warn(
            "task-index",
            format!(
                "baseline seeding finished with failures workspace={} failed={failed_count} total={}",
                ingest.workspace_path,
                candidates.len()
            ),
        );
    }
}

/// TS `buildBaselineMetaFromSummary`。
pub(super) fn baseline_meta(ingest: &Arc<Ingest>, summary: &Value) -> Value {
    let mut meta = json!({
        "taskId": summary["sessionId"],
        "traceId": uuid::Uuid::new_v4().to_string(),
        "title": summary["title"],
        "workspacePath": ingest.workspace_path,
        "createdAt": summary["createdAt"],
        "updatedAt": summary["lastActivityAt"],
        "mode": "default",
        "provider": AGENT_PROVIDER,
    });
    if summary["titleSource"] == "custom" {
        meta["titleOverridden"] = Value::Bool(true);
    }
    if let Some(identity) = &ingest.workspace_identity {
        meta["workspaceIdentity"] = Value::from(identity.clone());
    }
    if let Some(parent) = summary["parentSessionId"].as_str() {
        meta["forkedFromTaskId"] = Value::from(parent);
    }
    // TS `taskStatusFromSummaryPhase`。
    if let Some(status) = match summary["phase"].as_str() {
        Some("running" | "prewarming") => Some("running"),
        Some("completedSuccess" | "completedInterrupted") => Some("completed"),
        Some("error") => Some("error"),
        _ => None,
    } {
        meta["status"] = Value::from(status);
    }
    meta
}

/// TS `syncSnapshotAndBroadcast` 的索引部分（回源收敛路径；分组置顶差异见 spec §6.7.4-5）。
async fn sync_snapshot_to_index(
    ingest: &Arc<Ingest>,
    snapshot: &Value,
    reason: &str,
    unread_signal: Option<&str>,
    _move_to_top: bool,
) {
    if snapshot["session"]["sessionKind"] == "subagent_child" {
        // 只读详情不能重新写成主任务（TS `syncTaskIndexSnapshot` 同守卫）。
        return;
    }
    let meta = index_meta_from_snapshot(snapshot, &IndexOverrides::default());
    let searchable = searchable_text_from_snapshot(snapshot);
    let Some(record) = meta.as_object().cloned() else {
        return;
    };
    let read = ingest.read.clone();
    let persisted = read
        .run(move |index| {
            index
                .sync_task_meta(
                    &record,
                    &SyncOptions {
                        searchable_text: Some(searchable),
                        ..SyncOptions::default()
                    },
                )
                .map_err(|error| error.to_string())
        })
        .await;
    match persisted {
        Ok(persisted) => {
            remember_from_meta(&ingest.read.targets, &persisted);
            // 还没有用户可见内容的快照只写索引不广播（避免 "New session" 闪烁）。
            let session_id = persisted["taskId"].as_str().unwrap_or_default().to_owned();
            if has_user_visible_content(snapshot) && !session_id.is_empty() {
                ingest.events.task_list_changed(
                    &target_of(ingest, &persisted, &session_id),
                    Some(&persisted),
                    reason,
                    unread_signal,
                );
            }
        }
        Err((code, message)) => {
            logging::warn(
                "task-index",
                format!("snapshot sync failed: {code}: {message}"),
            );
        }
    }
}

fn target_of<'a>(ingest: &'a Arc<Ingest>, meta: &'a Value, session_id: &'a str) -> Target<'a> {
    Target {
        workspace_path: meta["workspacePath"]
            .as_str()
            .unwrap_or(&ingest.workspace_path),
        workspace_identity: meta["workspaceIdentity"]
            .as_str()
            .or(ingest.workspace_identity.as_deref()),
        task_id: Some(session_id),
    }
}

/// `applyAgentPatch` 的阻塞执行（行缺失回 `None` 由调用方决定回源）。
async fn patch_index(
    ingest: &Arc<Ingest>,
    session_id: &str,
    patch: AgentPatch,
) -> Result<Option<Value>, (String, String)> {
    let read = ingest.read.clone();
    let workspace_path = ingest.workspace_path.clone();
    let workspace_identity = ingest.workspace_identity.clone();
    let task_id = session_id.to_owned();
    read.run(move |index| {
        index
            .apply_agent_patch(
                &TaskRef {
                    workspace_path: &workspace_path,
                    workspace_identity: workspace_identity.as_deref(),
                    task_id: &task_id,
                },
                &patch,
            )
            .map_err(|error| error.to_string())
    })
    .await
}
