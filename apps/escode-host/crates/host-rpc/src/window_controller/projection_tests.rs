//! 投影状态机单测（TS `windowHostControllerProjection.ts` 的行为口径）：
//! 订阅/快照/增量水位、overlay 叠加、resync/unsubscribe 语义、行排序与 membership 过滤。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::FrameSink;
use super::{LocalScope, Projection, TASKS_TOPIC, WORKSPACES_TOPIC};
use crate::window_controller::rows::{matches_membership_kind, row_key};

fn projection() -> Projection {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    Projection::new(Box::new(move || {
        let next = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("id-{next}")
    }))
}

fn sink() -> (FrameSink, Arc<Mutex<Vec<Value>>>) {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&frames);
    let sink: FrameSink = Arc::new(move |frame| {
        collected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(frame);
    });
    (sink, frames)
}

fn frames(frames: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
    std::mem::take(
        &mut *frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

fn membership_entry(task_id: &str, updated_at: i64, flags: (bool, bool, bool)) -> (String, Value) {
    (
        row_key("/w", None, task_id),
        json!({
            "meta": {
                "taskId": task_id,
                "workspacePath": "/w",
                "title": format!("title-{task_id}"),
                "updatedAt": updated_at,
                "createdAt": updated_at,
            },
            "membership": {"pinned": flags.0, "archived": flags.1, "active": flags.2},
        }),
    )
}

fn memberships(entries: Vec<(String, Value)>) -> BTreeMap<String, Value> {
    entries.into_iter().collect()
}

fn scope() -> LocalScope {
    LocalScope::new("/w", None)
}

#[test]
fn subscription_delivers_a_snapshot_then_incremental_deltas() {
    let projection = projection();
    let (task_sink, task_frames) = sink();
    let ack = projection.subscribe(TASKS_TOPIC, task_sink).unwrap();
    assert_eq!(ack["ack"]["mode"], "snapshot");
    let subscription_id = ack["ack"]["subscriptionId"].as_str().unwrap().to_owned();

    let first = frames(&task_frames);
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0]["subscriptionId"],
        Value::from(subscription_id.clone())
    );
    assert_eq!(first[0]["fromSeq"], 0);
    assert_eq!(first[0]["toSeq"], 0);
    assert_eq!(first[0]["payload"]["kind"], "snapshot");
    assert!(
        first[0]["payload"]["snapshot"]["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        first[0]["payload"]["snapshot"]["logEpoch"],
        first[0]["logEpoch"]
    );

    // 首个快照替换：2 个 upsert（removals 为空）→ seq 0 → 1；workspace 事实 offline→online。
    let projection_ref = &projection;
    projection_ref.register_source(&scope());
    projection_ref.replace_source_snapshot(
        &scope(),
        memberships(vec![
            membership_entry("b", 20, (false, false, true)),
            membership_entry("a", 10, (false, false, true)),
        ]),
        BTreeMap::new(),
    );
    // 只订阅了 tasks topic：workspace 事实增量不投递给该订阅者（TS 按 topic 过滤）。
    let deltas = frames(&task_frames);
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    let tasks_frame = &deltas[0];
    assert_eq!(tasks_frame["topic"], TASKS_TOPIC);
    assert_eq!(tasks_frame["subscriptionId"], Value::from(subscription_id));
    assert_eq!(tasks_frame["fromSeq"], 0);
    assert_eq!(tasks_frame["toSeq"], 1);
    let ops: Vec<&str> = tasks_frame["payload"]["deltas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|delta| delta["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, vec!["task.upserted", "task.upserted"]);

    // workspaces topic 订阅者收到 workspace 事实（offline/connecting → online/online）。
    let (workspace_sink, workspace_frames) = sink();
    projection_ref
        .subscribe(WORKSPACES_TOPIC, workspace_sink)
        .unwrap();
    assert_eq!(
        frames(&workspace_frames)[0]["payload"]["snapshot"]["workspaces"][0]["sourceAvailability"],
        "online"
    );

    // 无变化 → 不发帧（deep-equal 才发 upserted）。
    projection_ref.replace_source_snapshot(
        &scope(),
        memberships(vec![
            membership_entry("b", 20, (false, false, true)),
            membership_entry("a", 10, (false, false, true)),
        ]),
        BTreeMap::new(),
    );
    assert!(
        frames(&task_frames).is_empty(),
        "no-change refresh publishes nothing"
    );

    // 一个任务消失：removals 先于 upserts，且 seq 递增。
    projection_ref.replace_source_snapshot(
        &scope(),
        memberships(vec![membership_entry("a", 10, (false, false, true))]),
        BTreeMap::new(),
    );
    let deltas = frames(&task_frames);
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    assert_eq!(deltas[0]["fromSeq"], 1);
    assert_eq!(deltas[0]["toSeq"], 2);
    let ops: Vec<&str> = deltas[0]["payload"]["deltas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|delta| delta["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, vec!["task.removed"]);
    assert_eq!(deltas[0]["payload"]["deltas"][0]["address"]["taskId"], "b");
}

#[test]
fn overlay_replacement_only_publishes_upserts() {
    let projection = projection();
    let (sink, collected) = sink();
    projection.subscribe(TASKS_TOPIC, sink).unwrap();
    projection.register_source(&scope());
    projection.replace_source_snapshot(
        &scope(),
        memberships(vec![membership_entry("t1", 10, (false, false, true))]),
        BTreeMap::new(),
    );
    frames(&collected);

    // overlay：waiting + 标题覆盖 + activity（phase=running → status=running）。
    let overlay = json!({
        "taskId": "t1",
        "liveStatus": "waiting",
        "title": "live title",
        "updatedAt": 42,
        "activity": {"phase": "running", "lastActivityAt": 42, "hasBackgroundWork": true},
    });
    projection.replace_source_overlays(&scope(), BTreeMap::from([("t1".to_owned(), overlay)]));
    let deltas = frames(&collected);
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    assert_eq!(deltas[0]["fromSeq"], 1);
    let task = &deltas[0]["payload"]["deltas"][0]["task"];
    assert_eq!(task["liveStatus"], "waiting");
    assert_eq!(task["meta"]["title"], "live title");
    assert_eq!(task["meta"]["updatedAt"], 42);
    assert_eq!(task["meta"]["status"], "running");
    assert_eq!(task["activity"]["hasBackgroundWork"], true);

    // 去掉 overlay：只发 upserted（没有 removal），liveStatus 退回 meta 派生值。
    projection.replace_source_overlays(&scope(), BTreeMap::new());
    let deltas = frames(&collected);
    assert_eq!(deltas.len(), 1, "{deltas:?}");
    let deltas_array = deltas[0]["payload"]["deltas"].as_array().unwrap();
    assert_eq!(deltas_array.len(), 1);
    assert_eq!(deltas_array[0]["op"], "task.upserted");
    assert_eq!(deltas_array[0]["task"]["liveStatus"], "idle");
    assert_eq!(deltas_array[0]["task"]["meta"]["title"], "title-t1");
    assert_eq!(deltas_array[0]["task"]["meta"]["updatedAt"], 10);
    assert!(deltas_array[0]["task"]["meta"].get("status").is_none());
    assert!(deltas_array[0]["task"].get("activity").is_none());
}

#[test]
fn overlay_title_and_pending_interaction_follow_the_ts_rules() {
    let projection = projection();
    projection.register_source(&scope());
    // meta 已手动改标题 → overlay 标题不覆盖（除非 titleSource === "custom"）。
    let mut entry = membership_entry("t1", 10, (false, false, true));
    entry.1["meta"]["titleOverridden"] = Value::Bool(true);
    entry.1["meta"]["pendingInteraction"] = json!({"interactionId": "old", "kind": "permission"});
    projection.replace_source_snapshot(&scope(), memberships(vec![entry]), BTreeMap::new());

    let overlay = |title_source: Value| {
        json!({
            "taskId": "t1",
            "liveStatus": "idle",
            "title": "overlay title",
            "titleSource": title_source,
        })
    };
    projection.replace_source_overlays(
        &scope(),
        BTreeMap::from([("t1".to_owned(), overlay(Value::Null))]),
    );
    let mut row = projection.rows(&[]).remove(0);
    assert_eq!(row["meta"]["title"], "title-t1");
    // overlay 无 pendingInteraction → meta 里的旧值整键消失（TS `undefined` 语义）。
    assert!(row["meta"].get("pendingInteraction").is_none());

    projection.replace_source_overlays(
        &scope(),
        BTreeMap::from([("t1".to_owned(), overlay(Value::from("custom")))]),
    );
    row = projection.rows(&[]).remove(0);
    assert_eq!(row["meta"]["title"], "overlay title");
    assert_eq!(row["meta"]["titleOverridden"], true);
}

#[test]
fn rows_sort_by_updated_at_then_row_key_and_membership_filters() {
    let projection = projection();
    projection.register_source(&scope());
    projection.replace_source_snapshot(
        &scope(),
        memberships(vec![
            membership_entry("a", 10, (false, false, true)),
            membership_entry("b", 30, (true, false, true)),
            membership_entry("c", 20, (false, true, false)),
            membership_entry("d", 30, (false, false, true)),
        ]),
        BTreeMap::new(),
    );
    let rows = projection.rows(&[]);
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["meta"]["taskId"].as_str().unwrap())
        .collect();
    // updatedAt 降序；同 updatedAt 用行键 localeCompare（b < d）。
    assert_eq!(ids, vec!["b", "d", "c", "a"]);

    let kind_ids = |kind: &str| -> Vec<String> {
        rows.iter()
            .filter(|row| matches_membership_kind(&row["membership"], kind))
            .map(|row| row["meta"]["taskId"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(kind_ids("pinned"), vec!["b"]);
    assert_eq!(kind_ids("archived"), vec!["c"]);
    assert_eq!(kind_ids("timeline"), vec!["d", "a"]);
    assert_eq!(kind_ids("active"), vec!["b", "d", "a"]);
}

#[test]
fn resync_and_unsubscribe_semantics() {
    let projection = projection();
    let (sink, collected) = sink();
    let ack = projection.subscribe(WORKSPACES_TOPIC, sink).unwrap();
    let subscription_id = ack["ack"]["subscriptionId"].as_str().unwrap().to_owned();
    frames(&collected);

    // 未订阅号：TS 同文案。
    let error = projection.resync("missing").unwrap_err();
    assert!(error.message.contains("未找到 Controller subscription"));

    // resync 重发快照（同一订阅号、同一 topic、fromSeq:0）。
    projection.resync(&subscription_id).unwrap();
    let resent = frames(&collected);
    assert_eq!(resent.len(), 1);
    assert_eq!(resent[0]["topic"], WORKSPACES_TOPIC);
    assert_eq!(
        resent[0]["subscriptionId"],
        Value::from(subscription_id.clone())
    );
    assert_eq!(resent[0]["fromSeq"], 0);

    // unsubscribe 幂等；之后不再有帧、resync 报错。
    projection.unsubscribe(&subscription_id);
    projection.unsubscribe(&subscription_id);
    projection.register_source(&scope());
    projection.replace_source_snapshot(&scope(), memberships(vec![]), BTreeMap::new());
    assert!(
        frames(&collected).is_empty(),
        "unsubscribed connection gets no frames"
    );
    assert!(projection.resync(&subscription_id).is_err());
}

#[test]
fn unknown_topic_and_mutation_validation_match_ts_errors() {
    let projection = projection();
    let (sink, _collected) = sink();
    assert!(projection.subscribe("controller/unknown", sink).is_err());

    let address = json!({"workspacePath": "/w", "taskId": "t1"});
    // 未登记的 source → "没有与任务地址匹配的 source"。
    let error = projection.validate_mutation(&address, false).unwrap_err();
    assert_eq!(error.message, "没有与任务地址匹配的 source");
    projection.register_source(&scope());
    // 离线 source（未快照）→ "本地 source 当前不可用"？TS 先判行存在，再判可用性。
    let error = projection.validate_mutation(&address, true).unwrap_err();
    assert_eq!(error.message, "本地 source 当前不可用");
    projection.replace_source_snapshot(
        &scope(),
        memberships(vec![membership_entry("t1", 10, (false, false, true))]),
        BTreeMap::new(),
    );
    // 行存在 → 通过；行缺失且不允许缺失 → 同一条 source 匹配错误。
    assert!(projection.validate_mutation(&address, false).is_ok());
    let missing = json!({"workspacePath": "/w", "taskId": "nope"});
    let error = projection.validate_mutation(&missing, false).unwrap_err();
    assert_eq!(error.message, "没有与任务地址匹配的 source");
    assert!(projection.validate_mutation(&missing, true).is_ok());
    // 远程地址本片不支持。
    let remote = json!({"remoteSessionId": "r1", "workspacePath": "/w",
        "workspaceIdentity": "remote:ssh:h:22:u:/w", "taskId": "t1"});
    assert!(projection.validate_mutation(&remote, true).is_err());
}
