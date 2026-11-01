//! 远端 scope 的未读信号（从 `remote_tests.rs` 拆出，守住测试文件行数）。

use serde_json::{Value, json};

use super::*;

#[tokio::test]
async fn background_terminal_completion_publishes_a_source_scoped_unread_signal() {
    use futures_util::StreamExt;

    let (client, _outbound) = initialized_client(&[TASK_CHANNEL]);
    let controller = RemoteController::new(client, &scope(IDENTITY));
    let mut signals = controller.unread().listen();
    let event = |reason: &str, signal: Option<&str>| {
        let mut event = json!({
            "type": "workspace_task_list_changed",
            "workspacePath": "/remote/workspace",
            "workspaceIdentity": IDENTITY,
            "taskId": "t1",
            "reason": reason,
            "taskMeta": {"taskId": "t1", "status": "completed", "updatedAt": 42},
        });
        if let Some(signal) = signal {
            event["unreadSignal"] = Value::from(signal);
        }
        event
    };
    // 同一 workspace 上的其它变更（pin/rename）不能变成未读提示。
    controller.publish_unread(&event("task_pinned", Some("background_terminal")));
    controller.publish_unread(&event("task_status_changed", None));
    controller.publish_unread(&event("task_status_changed", Some("background_terminal")));

    let wire = signals.next().await.expect("unread signal");
    match wire {
        crate::wire::Wire::Object(value) => {
            assert_eq!(value["address"]["taskId"], "t1");
            assert_eq!(value["address"]["sourceServerId"], SOURCE);
            assert_eq!(value["address"]["workspaceIdentity"], IDENTITY);
            assert_eq!(value["updatedAt"], json!(42));
            assert_eq!(value["status"], "completed");
        }
        other => panic!("unexpected wire: {other:?}"),
    }
    // 发送端随 controller 存活，流不会结束：用超时确认没有第二条信号。
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), signals.next())
            .await
            .is_err(),
        "只有 background_terminal 终态才发信号"
    );
}
