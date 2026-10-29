//! 逻辑帧应用与索引写（TS syncer 的 `applySessionsIndexFrame` / `processSummary` /
//! `applyTerminalTransition` / `applyTitleChange` / `seedMissingRowsFromInitialSnapshot` /
//! `resyncTaskIndexRowFromAgent` / `applyWorkspaceConfigFrame` / `syncSnapshotAndBroadcast`）。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::agent_v4::assembler::Delivery;
use crate::agent_v4::frames::TopicKind;

use super::Ingest;
use super::topic::{TopicState, complete_recovery_frame, handle_gap};

/// sessions-index 逻辑帧的最小形状（深校验差异见 spec §6.7.4-4）。
pub(crate) fn index_frame_check(frame: &Value) -> bool {
    frame.is_object()
        && frame["topic"].is_string()
        && frame["subscriptionId"].is_string()
        && frame["fromSeq"].is_u64()
        && frame["toSeq"].is_u64()
        && match frame["payload"]["kind"].as_str() {
            Some("snapshot") => {
                frame["payload"]["snapshot"]["logEpoch"].is_string()
                    && frame["payload"]["snapshot"]["sessions"].is_array()
            }
            Some("deltas") => frame["payload"]["deltas"].is_array(),
            _ => false,
        }
}

/// workspace-config 逻辑帧的最小形状。
pub(crate) fn workspace_config_frame_check(frame: &Value) -> bool {
    frame.is_object()
        && frame["topic"].is_string()
        && frame["subscriptionId"].is_string()
        && frame["fromSeq"].is_u64()
        && frame["toSeq"].is_u64()
        && match frame["payload"]["kind"].as_str() {
            Some("snapshot") => frame["payload"]["snapshot"]["config"].is_object(),
            Some("deltas") => frame["payload"]["deltas"].is_array(),
            _ => false,
        }
}

/// TS `applySessionsIndexFrame` / `applyWorkspaceConfigFrame` 的分派入口。
pub(super) fn apply_frame(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    frame: Value,
    delivery: Delivery,
) {
    match state.kind {
        TopicKind::SessionsIndex => apply_index_frame(ingest, state, frame, delivery),
        _ => apply_config_frame(ingest, state, frame, delivery),
    }
}

/// recovery 在飞时 online delta 只记 post-gap、不落基线（防错序；两 topic 同构）。
fn note_online_delta_during_recovery(state: &mut TopicState, to_seq: u64) {
    if let Some(recovery) = state.recovery.as_mut()
        && recovery.frame_applied
        && to_seq > state.seq
    {
        recovery.post_recovery_gap_pending = true;
    }
}

/// delta 无 base / 非法 delivery 时的 gap 分类（TS `deliveryKind === "recovery" ? … : "online"`）。
fn gap_delivery(delivery: Delivery) -> Option<Delivery> {
    Some(if delivery == Delivery::Recovery {
        Delivery::Recovery
    } else {
        Delivery::Online
    })
}

/// TS `applySessionsIndexFrame`。
fn apply_index_frame(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    frame: Value,
    delivery: Delivery,
) {
    let payload = frame["payload"].clone();
    let to_seq = frame["toSeq"].as_u64().unwrap_or_default();
    let from_seq = frame["fromSeq"].as_u64().unwrap_or_default();
    if delivery == Delivery::Online && state.recovery.is_some() && payload["kind"] == "deltas" {
        note_online_delta_during_recovery(state, to_seq);
        return;
    }
    if payload["kind"] == "snapshot" {
        if delivery == Delivery::Online && state.has_applied_base && to_seq <= state.seq {
            return;
        }
        state.log_epoch = payload["snapshot"]["logEpoch"].as_str().map(str::to_owned);
        state.seq = to_seq;
        state.has_applied_base = true;
        let next: Vec<Value> = payload["snapshot"]["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut core = super::lock(&ingest.core);
        if !core.seeded {
            // 首帧 = 静默基线：不回放历史终态、不发列表广播，仅原子补齐缺失行。
            let visible: std::collections::BTreeSet<String> = next
                .iter()
                .filter_map(|summary| summary["sessionId"].as_str().map(str::to_owned))
                .collect();
            core.summaries = sessions_map(next.iter());
            core.seeded = true;
            drop(core);
            // 首帧基线也是 `window-controller` 的 overlay 源（订阅前已存在的任务才有 overlay）。
            ingest.notify_overlays();
            complete_recovery_frame(ingest, state, delivery);
            let writer = Arc::clone(ingest);
            ingest.enqueue(async move { super::apply_writes::seed_missing_rows(&writer).await });
            // TS `repairSubagentTaskIndex`：旧索引里的 `subagent_child` 行要按 CLI 权威身份清扫。
            // 独立于写队列执行：CLI 请求失败/超时不能拖慢标题与状态写入；删除本身幂等。
            let repairer = Arc::clone(ingest);
            tokio::spawn(async move {
                super::repair::repair_subagent_rows(&repairer, &visible).await;
            });
            return;
        }
        // 断档降级 snapshot：对基线 diff 后处理（等价补投丢失的 deltas，conflated 语义）。
        let previous = core.summaries.clone();
        core.summaries.clear();
        for summary in &next {
            let id = summary["sessionId"].as_str().unwrap_or_default();
            process_summary(ingest, &mut core, previous.get(id), summary);
        }
        drop(core);
        ingest.notify_overlays();
        complete_recovery_frame(ingest, state, delivery);
        return;
    }
    if !state.has_applied_base {
        // fresh 订阅从不携带 base：initial 整批丢失后任何 delta 都不能建立 cold baseline。
        handle_gap(ingest, state, gap_delivery(delivery));
        return;
    }
    if to_seq <= state.seq {
        if delivery == Delivery::Recovery {
            // 已被更晚权威 snapshot/online 覆盖的 recovery：确认 delivery 成功。
            complete_recovery_frame(ingest, state, delivery);
        }
        return;
    }
    if from_seq != state.seq {
        handle_gap(ingest, state, Some(delivery));
        return;
    }
    {
        let mut core = super::lock(&ingest.core);
        for delta in payload["deltas"].as_array().map_or(&[][..], Vec::as_slice) {
            if delta["op"] == "session.upserted" {
                let id = delta["session"]["sessionId"].as_str().unwrap_or_default();
                let previous = core.summaries.get(id).cloned();
                process_summary(ingest, &mut core, previous.as_ref(), &delta["session"]);
            } else {
                // session.removed：删除的 sqlite 收口走 task 删除操作，这里只维护基线。
                core.summaries
                    .remove(delta["sessionId"].as_str().unwrap_or_default());
            }
        }
    }
    state.seq = to_seq;
    // overlay（liveStatus/activity/标题）随基线一并变化，通知 controller 投影刷新。
    ingest.notify_overlays();
    complete_recovery_frame(ingest, state, delivery);
}

fn sessions_map<'a>(sessions: impl Iterator<Item = &'a Value>) -> BTreeMap<String, Value> {
    sessions
        .map(|summary| {
            (
                summary["sessionId"].as_str().unwrap_or_default().to_owned(),
                summary.clone(),
            )
        })
        .collect()
}

/// TS `processSummary`：基线 diff 的分支（draft 跳过 / 终态迁移 / 新可见 / 标题变化）。
fn process_summary(
    ingest: &Arc<Ingest>,
    core: &mut super::Core,
    previous: Option<&Value>,
    summary: &Value,
) {
    let session_id = summary["sessionId"].as_str().unwrap_or_default().to_owned();
    core.summaries.insert(session_id.clone(), summary.clone());
    // draft 裁决：纯内存态、不落盘，也绝不进 task index。
    if summary["phase"] == "draft" {
        return;
    }
    let terminal = |phase: &Value| {
        matches!(
            phase.as_str(),
            Some("completedSuccess" | "completedInterrupted" | "error")
        )
    };
    let became_visible = previous.is_none_or(|previous| previous["phase"] == "draft");
    let became_terminal = previous.is_some_and(|previous| !terminal(&previous["phase"]))
        && terminal(&summary["phase"]);
    if became_terminal {
        // 远程 workspace replayable 的 host command 以 phase 终态为“下一条可发送”的边界：
        // 先收口当前 input（按 active traceId 移除 running 命令），再放行队列。
        let input_id = ingest.commands.session_terminal(
            &ingest.workspace_path,
            ingest.workspace_identity.as_deref(),
            &session_id,
        );
        // 终态事件先于索引写发出（TS `emitTerminalAndReady` 与索引收敛同源）。
        let error = (summary["phase"] == "error").then(|| {
            summary["lastError"]["message"]
                .as_str()
                .unwrap_or("Task failed")
                .to_owned()
        });
        ingest.task_events.terminal(
            &ingest.workspace_path,
            ingest.workspace_identity.as_deref(),
            &session_id,
            input_id.as_deref(),
            error.as_deref(),
        );
        let writer = Arc::clone(ingest);
        let summary = summary.clone();
        ingest.enqueue(async move {
            super::apply_writes::apply_terminal_transition(
                &writer,
                &session_id,
                &summary,
                became_visible,
            )
            .await;
        });
        return;
    }
    if became_visible {
        // 新任务边界（v4 预热提升 / online 首现）：立即回源建行。
        let writer = Arc::clone(ingest);
        ingest.enqueue(async move {
            super::apply_writes::resync_row(&writer, &session_id, true).await;
        });
        return;
    }
    let title = summary["title"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let title_changed = !title.is_empty()
        && previous.is_none_or(|previous| previous["title"].as_str() != Some(title.as_str()));
    if title_changed {
        let writer = Arc::clone(ingest);
        ingest.enqueue(async move {
            super::apply_writes::apply_title_change(&writer, &session_id, &title).await;
        });
    }
}

/// TS `resolveTerminalUnreadSignal`。
pub(super) fn resolve_terminal_unread_signal(summary: &Value) -> Option<&'static str> {
    if summary["phase"] == "error" {
        return Some("background_terminal");
    }
    if matches!(
        summary["phase"].as_str(),
        Some("completedSuccess" | "completedInterrupted")
    ) && matches!(summary["goalStatus"].as_str(), None | Some("verified"))
    {
        return Some("background_terminal");
    }
    None
}

/// TS `applyWorkspaceConfigFrame`：水位与 index topic 同构；目录非空才广播。
fn apply_config_frame(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    frame: Value,
    delivery: Delivery,
) {
    let payload = frame["payload"].clone();
    let to_seq = frame["toSeq"].as_u64().unwrap_or_default();
    let from_seq = frame["fromSeq"].as_u64().unwrap_or_default();
    if delivery == Delivery::Online && state.recovery.is_some() && payload["kind"] == "deltas" {
        note_online_delta_during_recovery(state, to_seq);
        return;
    }
    let config = if payload["kind"] == "snapshot" {
        if delivery == Delivery::Online && state.has_applied_base && to_seq <= state.seq {
            return;
        }
        state.log_epoch = payload["snapshot"]["logEpoch"].as_str().map(str::to_owned);
        state.seq = to_seq;
        state.has_applied_base = true;
        Some(payload["snapshot"]["config"].clone())
    } else {
        if !state.has_applied_base {
            handle_gap(ingest, state, gap_delivery(delivery));
            return;
        }
        if to_seq <= state.seq {
            if delivery == Delivery::Recovery {
                complete_recovery_frame(ingest, state, delivery);
            }
            return;
        }
        if from_seq != state.seq {
            handle_gap(ingest, state, Some(delivery));
            return;
        }
        state.seq = to_seq;
        payload["deltas"]
            .as_array()
            .and_then(|deltas| deltas.last())
            .map(|delta| delta["config"].clone())
    };
    let non_empty = config.as_ref().is_some_and(|config| {
        config["configOptions"]
            .as_array()
            .is_some_and(|options| !options.is_empty())
    });
    complete_recovery_frame(ingest, state, delivery);
    if !non_empty {
        // 空目录不下发：下游收到空 configOptions 会把模型目录清掉。
        return;
    }
    let mut event = json!({
        "type": "workspace_config_options_update",
        "workspacePath": ingest.workspace_path,
        "configOptions": config.expect("checked non-empty above")["configOptions"],
    });
    if let Some(identity) = &ingest.workspace_identity {
        event["workspaceIdentity"] = Value::from(identity.clone());
    }
    ingest.events.emit(&ingest.key, &event);
}
