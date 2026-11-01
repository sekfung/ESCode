//! 未读信号派生：只认 `background_terminal` 的完成/失败，远端身份不重复认领。

use futures_util::StreamExt;
use serde_json::json;

use super::*;
use crate::workspace_events::{Target, WorkspaceEvents};

fn event(reason: &str, signal: Option<&str>, status: Option<&str>) -> serde_json::Value {
    let mut event = json!({
        "type": "workspace_task_list_changed",
        "workspacePath": "/w",
        "workspaceIdentity": "local:/w",
        "taskId": "t1",
        "reason": reason,
    });
    if let Some(signal) = signal {
        event["unreadSignal"] = serde_json::Value::from(signal);
    }
    if let Some(status) = status {
        event["taskMeta"] =
            json!({"taskId": "t1", "status": status, "updatedAt": 1_700_000_000_000_i64});
    }
    event
}

#[test]
fn background_terminal_completion_becomes_one_signal() {
    for status in ["completed", "error"] {
        let signal = signal_of(&event(
            "task_status_changed",
            Some("background_terminal"),
            Some(status),
        ))
        .unwrap_or_else(|| panic!("{status} should signal"));
        assert_eq!(signal["address"]["taskId"], "t1");
        assert_eq!(signal["address"]["workspacePath"], "/w");
        assert_eq!(signal["address"]["workspaceIdentity"], "local:/w");
        assert_eq!(signal["updatedAt"], json!(1_700_000_000_000_i64));
        assert_eq!(signal["status"], status);
    }
}

#[test]
fn other_reasons_statuses_and_missing_meta_do_not_signal() {
    assert!(
        signal_of(&event(
            "task_pinned",
            Some("background_terminal"),
            Some("completed")
        ))
        .is_none()
    );
    assert!(signal_of(&event("task_status_changed", None, Some("completed"))).is_none());
    assert!(
        signal_of(&event(
            "task_status_changed",
            Some("background_terminal"),
            Some("running")
        ))
        .is_none()
    );
    assert!(
        signal_of(&event(
            "task_status_changed",
            Some("background_terminal"),
            Some("completed")
        ))
        .is_some()
    );
    let mut missing_meta = event("task_status_changed", Some("background_terminal"), None);
    missing_meta["taskMeta"] = serde_json::Value::Null;
    assert!(signal_of(&missing_meta).is_none());
}

#[test]
fn remote_identity_is_left_to_the_connection_scoped_controller() {
    let mut remote = event(
        "task_status_changed",
        Some("background_terminal"),
        Some("completed"),
    );
    remote["workspaceIdentity"] = serde_json::Value::from("remote:ssh:host:22:dev:/w");
    assert!(super::super::is_remote_workspace_identity(
        "remote:ssh:host:22:dev:/w"
    ));
    assert!(local_signal_of(&remote).is_none());
    // 连接级 controller 自己发同形状信号（补 sourceServerId），形状派生不看归属。
    assert!(signal_of(&remote).is_some());
}

#[tokio::test]
async fn listen_forwards_signals_from_the_workspace_bus() {
    let events = WorkspaceEvents::default();
    let mut stream = listen(events.listen_all());
    events.task_list_changed(
        &Target {
            workspace_path: "/w",
            workspace_identity: Some("local:/w"),
            task_id: Some("t1"),
        },
        Some(&json!({"taskId": "t1", "status": "completed", "updatedAt": 7})),
        "task_status_changed",
        Some("background_terminal"),
    );
    let wire = stream.next().await.expect("signal");
    match wire {
        crate::wire::Wire::Object(value) => {
            assert_eq!(value["address"]["taskId"], "t1");
            assert_eq!(value["status"], "completed");
        }
        other => panic!("unexpected wire: {other:?}"),
    }
}
