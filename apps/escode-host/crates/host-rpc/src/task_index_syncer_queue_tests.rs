//! syncer → host command queue 的终态联动（S8 从 `task_index_syncer_tests.rs` 拆出，
//! 守住单文件行数上限）。

use std::sync::Arc;

use serde_json::json;

use super::tests::{index_wire, ingest_with_state, snapshot_payload, summary};
use super::topic as sync_topic;
use crate::agent_v4::frames::TopicKind;

/// syncer 的 phase 终态是 host command queue 的 ready 边界：终态后 owner 校验必须回到
/// `NO_ACTIVE_TASK_OWNER`（active 已清除），不能继续放行旧 run 的命令。
#[tokio::test(flavor = "multi_thread")]
async fn terminal_transition_clears_the_host_command_owner() {
    let queue = Arc::new(crate::task_commands::TaskCommandQueue::new());
    let (_dir, ingest, _events, queue, _task_events) = ingest_with_state(Arc::clone(&queue));
    sync_topic::set_subscription_for_tests(&ingest, TopicKind::SessionsIndex, "sub-1", "e1");
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            0,
            1,
            "initial",
            snapshot_payload(vec![summary("s1", "running", "首条输入")]),
        ),
    );

    let target = crate::task_targets::TaskTarget {
        task_id: "s1".to_owned(),
        workspace_path: "/w".to_owned(),
        workspace_identity: None,
        cron_automation_id: None,
        remote_session_id: None,
    };
    queue.set_active(&target, "input-1");
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            1,
            2,
            "online",
            json!({"kind": "deltas", "deltas": [
                {"op": "session.upserted", "session": summary("s1", "completedSuccess", "首条输入")},
            ]}),
        ),
    );

    let error = queue
        .enqueue(
            &target,
            &json!({
                "commandId": "cmd-2",
                "traceId": "input-2",
                "type": "send_prompt",
                "content": "second",
                "ownerRunId": "input-1",
            }),
        )
        .expect_err("terminal must clear the active input");
    assert_eq!(error.extra["code"], "NO_ACTIVE_TASK_OWNER");
}

/// syncer 终态同时驱动事件面：inputId 来自 queue active，complete/ready 都按 taskId 投递。
#[tokio::test(flavor = "multi_thread")]
async fn terminal_transition_emits_task_events() {
    use futures_util::StreamExt;

    let queue = Arc::new(crate::task_commands::TaskCommandQueue::new());
    let (_dir, ingest, _events, queue, task_events) = ingest_with_state(Arc::clone(&queue));
    let mut stream = task_events.listen_stream("s1");
    let mut ready = task_events.listen_ready("s1");
    sync_topic::set_subscription_for_tests(&ingest, TopicKind::SessionsIndex, "sub-1", "e1");
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            0,
            1,
            "initial",
            snapshot_payload(vec![summary("s1", "running", "首条输入")]),
        ),
    );
    let target = crate::task_targets::TaskTarget {
        task_id: "s1".to_owned(),
        workspace_path: "/w".to_owned(),
        workspace_identity: None,
        cron_automation_id: None,
        remote_session_id: None,
    };
    queue.set_active(&target, "input-1");
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            1,
            2,
            "online",
            json!({"kind": "deltas", "deltas": [
                {"op": "session.upserted", "session": summary("s1", "completedSuccess", "首条输入")},
            ]}),
        ),
    );

    let Some(crate::wire::Wire::Object(complete)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("event in time")
    else {
        panic!("task_complete expected")
    };
    assert_eq!(complete["type"], "task_complete");
    assert_eq!(complete["inputId"], "input-1");
    let Some(crate::wire::Wire::Object(ready)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), ready.next())
            .await
            .expect("ready in time")
    else {
        panic!("ready expected")
    };
    assert_eq!(ready["reason"], "prompt_completed");
}
