//! syncer 的语义边界：水位/恢复状态机、基线播种、终态/标题收敛（运行时链路由
//! `tests/rpc_task_sync.rs` 端到端覆盖）。
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::agent_v4::base::Base;
use crate::agent_v4::frames::TopicKind;
use crate::domains_task_support::{LazyIndex, ReadContext};
use crate::task_index_syncer::{Core, Ingest, Workspace};
use crate::task_targets::TaskTargets;
use crate::workspace_events::WorkspaceEvents;

// topic 模块的测试注入助手（pub(super) 只对 task_index_syncer 子树可见）。
use crate::task_index_syncer::topic as sync_topic;

pub(super) fn ingest_with_index() -> (tempfile::TempDir, Arc<Ingest>, crate::session::EventStream) {
    let (dir, ingest, events, _, _) =
        ingest_with_state(Arc::new(crate::task_commands::TaskCommandQueue::new()));
    (dir, ingest, events)
}

pub(super) fn ingest_with_state(
    commands: Arc<crate::task_commands::TaskCommandQueue>,
) -> (
    tempfile::TempDir,
    Arc<Ingest>,
    crate::session::EventStream,
    Arc<crate::task_commands::TaskCommandQueue>,
    Arc<crate::task_events::TaskEventHub>,
) {
    let task_events = Arc::new(crate::task_events::TaskEventHub::default());
    let dir = tempfile::tempdir().unwrap();
    let data_base = dir.path().join("data-base");
    std::fs::create_dir_all(data_base.join(".escode").join("v2")).unwrap();
    let index_path = data_base
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite");
    let connection = rusqlite::Connection::open(&index_path).unwrap();
    escode_host_services::tasks_index::fixture::apply_generated_schema(
        &connection,
        escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS,
    )
    .unwrap();
    drop(connection);
    let read = ReadContext {
        path: index_path,
        index: Arc::new(std::sync::Mutex::new(LazyIndex::default())),
        targets: Arc::new(TaskTargets::new()),
        runtime: None,
    };
    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory = Arc::new(|_| unreachable!("no runtime"));
    let plugin_management =
        crate::domains_plugin::PluginManagement::new(instances.clone(), options.clone(), None);
    let base = Arc::new(Base::new(
        instances,
        options,
        None,
        None,
        plugin_management,
        None,
    ));
    let events = Arc::new(WorkspaceEvents::default());
    let event_stream = events.listen(&json!("/w"));
    let (overlay_changes, _) = tokio::sync::broadcast::channel(8);
    (
        dir,
        Arc::new(Ingest::new(
            base,
            read,
            events,
            Arc::clone(&commands),
            Arc::clone(&task_events),
            "test-conn#task-index".to_owned(),
            Workspace {
                path: "/w".to_owned(),
                identity: None,
                key: "/w".to_owned(),
            },
            overlay_changes,
        )),
        event_stream,
        commands,
        task_events,
    )
}
pub(super) fn summary(id: &str, phase: &str, title: &str) -> Value {
    json!({
        "sessionId": id,
        "workspaceId": "/w",
        "title": title,
        "phase": phase,
        "sessionEnded": false,
        "hasBackgroundWork": false,
        "lastActivityAt": 100,
        "createdAt": 90,
    })
}

pub(super) fn index_wire(sub: &str, from: u64, to: u64, delivery: &str, payload: Value) -> Value {
    index_wire_ord(sub, from, to, to, delivery, payload)
}

/// `ordinal` 是装配层 tombstone 序（必须单调），与水位 seq 无关。
fn index_wire_ord(
    sub: &str,
    from: u64,
    to: u64,
    ordinal: u64,
    delivery: &str,
    payload: Value,
) -> Value {
    json!({
        "wireVersion": 3,
        "kind": "complete",
        "deliveryKind": delivery,
        "logicalFrameId": format!("lf-{ordinal}"),
        "logicalFrameOrdinal": ordinal,
        "topic": "sessions-index//w",
        "subscriptionId": sub,
        "frame": {
            "topic": "sessions-index//w",
            "subscriptionId": sub,
            "fromSeq": from,
            "toSeq": to,
            "sentAt": 0,
            "payload": payload,
        },
    })
}

pub(super) fn snapshot_payload(sessions: Vec<Value>) -> Value {
    json!({
        "kind": "snapshot",
        "snapshot": {"protocolVersion": 1, "workspaceId": "/w", "logEpoch": "e1", "sessions": sessions},
    })
}

pub(super) async fn wait_for<T>(mut query: impl FnMut() -> Option<T>, what: &str) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = query() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn first_snapshot_seeds_silently_then_terminal_and_title_converge() {
    let (_dir, ingest, mut events) = ingest_with_index();
    sync_topic::set_subscription_for_tests(&ingest, TopicKind::SessionsIndex, "sub-1", "e1");

    // 首帧 snapshot：静默播种（行出现、无广播、无终态回放）。
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            0,
            3,
            "initial",
            snapshot_payload(vec![summary("s1", "running", "首条输入")]),
        ),
    );
    {
        let core: std::sync::MutexGuard<'_, Core> = sync_topic::core_for_tests(&ingest);
        assert!(core.seeded);
        assert_eq!(core.summaries.len(), 1);
    }
    let seeded = wait_for(|| task_meta_sync(&ingest, "s1"), "baseline seed row").await;
    assert_eq!(seeded["status"], "running");
    assert_eq!(seeded["provider"], "glm");
    assert_eq!(seeded["mode"], "default");
    assert!(
        tokio::time::timeout(Duration::from_millis(120), events.next())
            .await
            .is_err(),
        "baseline seeding must not broadcast"
    );

    // delta：终态迁移 → 索引 completed + task_status_changed（无未验证 goal → 未读提醒）。
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            3,
            4,
            "online",
            json!({"kind": "deltas", "deltas": [
                {"op": "session.upserted", "session": summary("s1", "completedSuccess", "首条输入")},
            ]}),
        ),
    );
    let completed = wait_for(
        || task_meta_sync(&ingest, "s1").filter(|meta| meta["status"] == "completed"),
        "terminal status",
    )
    .await;
    assert_eq!(completed["title"], "首条输入");
    let event = next_event(&mut events, "task_status_changed").await;
    assert_eq!(event["taskId"], "s1");
    assert_eq!(event["unreadSignal"], "background_terminal");

    // delta：标题变化 → 索引标题 + task_title_changed。
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            4,
            5,
            "online",
            json!({"kind": "deltas", "deltas": [
                {"op": "session.upserted", "session": summary("s1", "completedSuccess", "自动标题")},
            ]}),
        ),
    );
    let renamed = wait_for(
        || task_meta_sync(&ingest, "s1").filter(|meta| meta["title"] == "自动标题"),
        "agent title",
    )
    .await;
    assert_eq!(renamed["titleOverridden"], false);
    let event = next_event(&mut events, "task_title_changed").await;
    assert_eq!(event["taskMeta"]["title"], "自动标题");
}

/// 同步轮询入口（wait_for 的查询闭包需要 FnMut，这里包一层阻塞读）。
pub(super) fn task_meta_sync(ingest: &Arc<Ingest>, task_id: &str) -> Option<Value> {
    let read = ingest.read.clone();
    let task_id = task_id.to_owned();
    // 阻塞线程上取锁读一行（测试轮询专用；实现路径走 read.run）。
    let mut guard = crate::task_index_syncer::lock(&read.index);
    guard
        .with(&read.path, |index| {
            index
                .get_task_meta("/w", None, &task_id)
                .map_err(|e| e.to_string())
        })
        .expect("index open")
}

/// 等待下一条事件（顺序消费；断言 reason 匹配）。
pub(super) async fn next_event(events: &mut crate::session::EventStream, reason: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for event {reason}"
        );
        match tokio::time::timeout(Duration::from_millis(100), events.next()).await {
            Ok(Some(wire)) => {
                let crate::wire::Wire::Object(event) = wire else {
                    continue;
                };
                assert_eq!(event["reason"], reason, "unexpected event {event}");
                return event;
            }
            Ok(None) => panic!("event stream closed while waiting for {reason}"),
            Err(_) => continue,
        }
    }
}
#[tokio::test]
async fn watermark_rules_guard_the_baseline() {
    let (_dir, ingest, _events) = ingest_with_index();
    sync_topic::set_subscription_for_tests(&ingest, TopicKind::SessionsIndex, "sub-1", "e1");

    // 无 base 的 delta 不能建立 cold baseline：不落摘要，且同步进入恢复态（强制 snapshot）。
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            0,
            1,
            "online",
            json!({"kind": "deltas", "deltas": [
                {"op": "session.upserted", "session": summary("s1", "running", "x")},
            ]}),
        ),
    );
    {
        let state = ingest.state(TopicKind::SessionsIndex);
        assert!(!state.has_applied_base);
        let core = sync_topic::core_for_tests(&ingest);
        assert!(!core.seeded);
        assert!(state.recovery.is_some(), "gap must enter recovery");
        assert!(state.recovery.as_ref().unwrap().force_snapshot);
    }
    sync_topic::discard_recovery(&mut ingest.state(TopicKind::SessionsIndex));

    // 已有 base：重复帧（toSeq ≤ seq）静默；gap（fromSeq ≠ seq）触发恢复；正常 delta 推进。
    {
        let mut state = ingest.state(TopicKind::SessionsIndex);
        state.has_applied_base = true;
        state.seq = 5;
    }
    let core = sync_topic::core_for_tests(&ingest);
    drop(core);
    sync_topic::core_for_tests(&ingest)
        .summaries
        .insert("s1".to_owned(), summary("s1", "running", "x"));
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            3,
            4,
            "online",
            json!({"kind": "deltas", "deltas": []}),
        ),
    );
    assert_eq!(
        ingest.state(TopicKind::SessionsIndex).seq,
        5,
        "stale delta ignored"
    );
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            7,
            9,
            "online",
            json!({"kind": "deltas", "deltas": []}),
        ),
    );
    assert!(
        ingest.state(TopicKind::SessionsIndex).recovery.is_some(),
        "seq gap must trigger recovery"
    );
    assert!(
        !ingest
            .state(TopicKind::SessionsIndex)
            .recovery
            .as_ref()
            .unwrap()
            .force_snapshot,
        "resync with an applied base resumes from the watermark"
    );
    // recovery 在飞时 online delta：未收口不置 post-gap；收口后更高的 toSeq 置 post-gap。
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            5,
            6,
            "online",
            json!({"kind": "deltas", "deltas": []}),
        ),
    );
    assert!(
        !ingest
            .state(TopicKind::SessionsIndex)
            .recovery
            .as_ref()
            .unwrap()
            .post_recovery_gap_pending,
        "no post-gap before the recovery frame lands"
    );
    ingest
        .state(TopicKind::SessionsIndex)
        .recovery
        .as_mut()
        .expect("recovery in flight")
        .frame_applied = true;
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire_ord(
            "sub-1",
            6,
            8,
            13,
            "online",
            json!({"kind": "deltas", "deltas": []}),
        ),
    );
    assert!(
        ingest
            .state(TopicKind::SessionsIndex)
            .recovery
            .as_ref()
            .unwrap()
            .post_recovery_gap_pending,
        "post-recovery online gap must be recorded"
    );
}
