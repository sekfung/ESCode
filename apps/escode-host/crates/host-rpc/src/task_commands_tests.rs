//! 命令队列状态机（S8）：字段形状、owner 三态、提升/取消、终态收口与 drain 闸门。

use super::*;

fn target() -> TaskTarget {
    TaskTarget {
        task_id: "task-1".to_owned(),
        workspace_path: "/repo".to_owned(),
        workspace_identity: Some(" ssh:host ".to_owned()),
        cron_automation_id: None,
        remote_session_id: None,
    }
}

fn params(command_id: &str, trace_id: &str, content: &str) -> Value {
    json!({
        "commandId": command_id,
        "traceId": trace_id,
        "queryId": format!("query-{trace_id}"),
        "clientId": "phone-1",
        "clientLabel": "远程",
        "content": content,
    })
}

fn code(error: &RpcError) -> &str {
    error.extra["code"].as_str().unwrap_or_default()
}

#[test]
fn enqueue_records_the_ts_command_shape() {
    let queue = Arc::new(TaskCommandQueue::new());
    let target = target();
    let mut params = params("cmd-1", "input-2", "second");
    params["automationId"] = json!("automation-1");
    params["attachments"] = json!([{"id": "a1"}]);
    let result = queue.enqueue(&target, &params).expect("enqueue");
    assert_eq!(result["accepted"], true);
    let command = &result["command"];
    assert_eq!(command["commandId"], "cmd-1");
    assert_eq!(command["taskId"], "task-1");
    assert_eq!(command["traceId"], "input-2");
    assert_eq!(command["queryId"], "query-input-2");
    // 事件与索引都用身份键：identity trim 后优先于路径。
    assert_eq!(command["workspaceKey"], "ssh:host");
    assert_eq!(command["workspaceIdentity"], " ssh:host ");
    assert_eq!(command["status"], "accepted");
    assert_eq!(command["type"], "send_prompt");
    assert_eq!(command["content"], "second");
    assert_eq!(command["automationId"], "automation-1");
    assert_eq!(command["clientId"], "phone-1");
    assert_eq!(command["clientLabel"], "远程");
    assert_eq!(command["attachments"][0]["id"], "a1");
    assert_eq!(command["createdAt"], command["updatedAt"]);
    assert_eq!(queue.pending(&target), vec![command.clone()]);
}

#[test]
fn owner_guard_rejects_missing_and_stale_active_runs() {
    let queue = Arc::new(TaskCommandQueue::new());
    let target = target();
    let mut owned = params("cmd-1", "input-1", "first");
    owned["ownerRunId"] = json!("run-1");
    let error = queue.enqueue(&target, &owned).expect_err("no active run");
    assert_eq!(error.message, "No active task owner.");
    assert_eq!(code(&error), "NO_ACTIVE_TASK_OWNER");

    queue.set_active(&target, "run-1");
    owned["ownerRunId"] = json!("run-2");
    let error = queue.enqueue(&target, &owned).expect_err("stale owner");
    assert_eq!(error.message, "Stale task owner command.");
    assert_eq!(code(&error), "STALE_TASK_OWNER_COMMAND");

    owned["ownerRunId"] = json!("run-1");
    assert!(queue.enqueue(&target, &owned).is_ok());
}

#[test]
fn promote_moves_to_the_front_and_cancel_is_idempotent() {
    let queue = Arc::new(TaskCommandQueue::new());
    let target = target();
    queue
        .enqueue(&target, &params("cmd-1", "input-1", "first"))
        .unwrap();
    queue
        .enqueue(&target, &params("cmd-2", "input-2", "second"))
        .unwrap();
    let promoted = queue
        .promote(&target, &json!({"commandId": "cmd-2"}))
        .expect("promote");
    assert_eq!(promoted["accepted"], true);
    assert_eq!(promoted["command"]["status"], "accepted");
    let pending = queue.pending(&target);
    assert_eq!(pending[0]["commandId"], "cmd-2");
    assert_eq!(pending[1]["commandId"], "cmd-1");

    let canceled = queue
        .cancel(&target, &json!({"commandId": "cmd-2"}))
        .expect("cancel");
    assert_eq!(
        canceled,
        json!({
            "canceled": true,
            "commandId": "cmd-2",
            "status": "accepted",
        })
    );
    assert_eq!(queue.pending(&target).len(), 1);
    // 已删除的命令再取消是幂等 not_found，不报错。
    let again = queue
        .cancel(&target, &json!({"commandId": "cmd-2"}))
        .expect("cancel again");
    assert_eq!(again["reason"], "not_found");
    assert_eq!(again["canceled"], true);

    let error = queue
        .promote(&target, &json!({"commandId": "missing"}))
        .expect_err("missing command");
    assert_eq!(error.message, "Task command not found.");
    assert_eq!(code(&error), "OWNER_COMMAND_FAILED");
}

#[test]
fn active_run_blocks_drain_and_only_the_matching_running_command_is_released() {
    let queue = Arc::new(TaskCommandQueue::new());
    let target = target();
    queue
        .enqueue(&target, &params("cmd-1", "input-1", "first"))
        .unwrap();
    queue
        .enqueue(&target, &params("cmd-2", "input-2", "second"))
        .unwrap();

    queue.set_active(&target, "input-0");
    assert!(queue.take_next(&target).is_none(), "active 期间不取命令");

    // 模拟 sendPrompt 已接受 input-1：drain 取走它并标 running。
    queue.set_active(&target, "input-1");
    queue.clear_active(&target);
    let running = queue.take_next(&target).expect("first command");
    assert_eq!(running["commandId"], "cmd-1");
    assert_eq!(running["status"], "running");
    assert!(queue.take_next(&target).is_none(), "running 期间不再取");

    let canceled = queue
        .cancel(&target, &json!({"commandId": "cmd-1"}))
        .expect("cancel");
    assert_eq!(canceled["canceled"], false);
    assert_eq!(canceled["reason"], "already_running");

    queue.set_active(&target, "input-1");
    queue.session_terminal("/repo", Some(" ssh:host "), "task-1");
    let pending = queue.pending(&target);
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["commandId"], "cmd-2");
    assert_eq!(pending[0]["status"], "accepted");
}

#[test]
fn terminal_without_matching_input_keeps_commands() {
    let queue = Arc::new(TaskCommandQueue::new());
    let target = target();
    queue
        .enqueue(&target, &params("cmd-1", "input-1", "first"))
        .unwrap();
    let running = queue.take_next(&target).expect("first command");
    assert_eq!(running["status"], "running");

    // active 属于另一个 input（例如队列外的轮次）：不误删 running 命令。
    queue.set_active(&target, "input-other");
    queue.session_terminal("/repo", Some(" ssh:host "), "task-1");
    assert_eq!(queue.pending(&target).len(), 1);
}
