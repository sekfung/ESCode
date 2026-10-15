//! topic 水位与帧构造（TS `windowHostControllerProjection.ts` 的 `snapshotFrame` /
//! `publishTaskDeltas` / `publishWorkspaceDeltas` / `allRows`）。从 `projection.rs` 拆出。

use serde_json::{Value, json};

use super::projection::{State, TASKS_TOPIC, TopicState, WORKSPACES_TOPIC};
use super::rows::{now_ms, workspace_fact};
use crate::collation;
use crate::session::RpcError;

pub(super) fn ack(subscription_id: &str, log_epoch: &str) -> Value {
    json!({"ack": {
        "subscriptionId": subscription_id,
        "mode": "snapshot",
        "logEpoch": log_epoch,
    }})
}

/// `publishTaskDeltas`：空批不发帧；非空则 `seq+=1` 并对该 topic 的全部订阅者各发一帧。
pub(super) fn publish_task_deltas(state: &mut State, deltas: Vec<Value>) {
    if deltas.is_empty() {
        return;
    }
    let from_seq = state.tasks.seq;
    let seq = from_seq + 1;
    state.tasks.seq = seq;
    let frame = json!({
        "topic": TASKS_TOPIC,
        "logEpoch": state.tasks.log_epoch,
        "fromSeq": from_seq,
        "toSeq": seq,
        "sentAt": now_ms(),
        "payload": {"kind": "deltas", "deltas": deltas},
    });
    publish(state, TASKS_TOPIC, frame);
}

pub(super) fn publish_workspace_deltas(state: &mut State, deltas: Vec<Value>) {
    if deltas.is_empty() {
        return;
    }
    let from_seq = state.workspaces.seq;
    let seq = from_seq + 1;
    state.workspaces.seq = seq;
    let frame = json!({
        "topic": WORKSPACES_TOPIC,
        "logEpoch": state.workspaces.log_epoch,
        "fromSeq": from_seq,
        "toSeq": seq,
        "sentAt": now_ms(),
        "payload": {"kind": "deltas", "deltas": deltas},
    });
    publish(state, WORKSPACES_TOPIC, frame);
}

pub(super) fn publish(state: &State, topic: &'static str, frame: Value) {
    // TS `publishTaskDeltas`：同一批增量对每个订阅者各发一帧，`subscriptionId` 是订阅者自己的。
    for (subscription_id, subscriber) in &state.subscribers {
        if subscriber.topic == topic {
            let mut frame = frame.clone();
            frame["subscriptionId"] = Value::from(subscription_id.clone());
            (subscriber.sink)(frame);
        }
    }
}

/// 快照帧（`fromSeq:0`、`payload.snapshot.logEpoch` 与帧同）。
pub(super) fn snapshot_frame(state: &State, topic: &'static str, subscription_id: &str) -> Value {
    let topic_state = topic_state(state, topic);
    let payload = if topic == TASKS_TOPIC {
        json!({"kind": "snapshot", "snapshot": {
            "protocolVersion": 1,
            "logEpoch": topic_state.log_epoch,
            "tasks": sorted_rows(state, &[]),
        }})
    } else {
        json!({"kind": "snapshot", "snapshot": {
            "protocolVersion": 1,
            "logEpoch": topic_state.log_epoch,
            "workspaces": workspace_facts(state),
        }})
    };
    json!({
        "topic": topic,
        "subscriptionId": subscription_id,
        "logEpoch": topic_state.log_epoch,
        "fromSeq": 0,
        "toSeq": topic_state.seq,
        "sentAt": now_ms(),
        "payload": payload,
    })
}

pub(super) fn topic_name(topic: &str) -> Result<&'static str, RpcError> {
    // 用比较而非 match 常量：`&str` 常量在跨模块 pattern 位置会被当成新绑定（实测）。
    if topic == TASKS_TOPIC {
        return Ok(TASKS_TOPIC);
    }
    if topic == WORKSPACES_TOPIC {
        return Ok(WORKSPACES_TOPIC);
    }
    Err(RpcError::new(
        "InvalidRequest",
        format!("Unknown controller topic `{topic}`"),
    ))
}

pub(super) fn topic_state<'a>(state: &'a State, topic: &'static str) -> &'a TopicState {
    if topic == TASKS_TOPIC {
        &state.tasks
    } else {
        &state.workspaces
    }
}

/// TS `allRows()` / `getTasks()`：跨 source 的行，`updatedAt` 降序 → 行键 `localeCompare`。
pub(super) fn sorted_rows(state: &State, source_keys: &[String]) -> Vec<Value> {
    let mut rows: Vec<(String, Value)> = Vec::new();
    for (key, source) in &state.sources {
        if !source_keys.is_empty() && !source_keys.contains(key) {
            continue;
        }
        rows.extend(
            source
                .rows
                .iter()
                .map(|(row_key, row)| (row_key.clone(), row.clone())),
        );
    }
    rows.sort_by(|(left_key, left), (right_key, right)| {
        let left_at = left["meta"]["updatedAt"].as_i64().unwrap_or(0);
        let right_at = right["meta"]["updatedAt"].as_i64().unwrap_or(0);
        right_at
            .cmp(&left_at)
            .then_with(|| collation::compare_names(left_key, right_key))
    });
    rows.into_iter().map(|(_, row)| row).collect()
}

/// TS `allWorkspaceFacts()`：按 `<remoteSessionId ?? "local">\0<identity ?? path>` 排序。
fn workspace_facts(state: &State) -> Vec<Value> {
    let mut facts: Vec<(String, Value)> = state
        .sources
        .values()
        .map(|source| {
            let sort_key = format!("local\0{}", source.scope.workspace_key());
            (sort_key, workspace_fact(source))
        })
        .collect();
    facts.sort_by(|(left, _), (right, _)| collation::compare_names(left, right));
    facts.into_iter().map(|(_, fact)| fact).collect()
}
