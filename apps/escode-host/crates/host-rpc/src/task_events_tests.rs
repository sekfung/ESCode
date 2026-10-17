use super::*;
use futures_util::StreamExt;

fn target() -> TaskTarget {
    TaskTarget {
        task_id: "t1".to_owned(),
        workspace_path: "/w".to_owned(),
        workspace_identity: Some("ssh:host".to_owned()),
        cron_automation_id: None,
        remote_session_id: None,
    }
}

async fn next(stream: &mut crate::session::EventStream) -> Value {
    match stream.next().await {
        Some(Wire::Object(value)) => value,
        other => panic!("event: {other:?}"),
    }
}

#[tokio::test]
async fn terminal_emits_stream_snapshot_ready_and_outcome() {
    let hub = Arc::new(TaskEventHub::default());
    let mut stream = hub.listen_stream("t1");
    let mut ready = hub.listen_ready("t1");
    let mut terminal = hub.listen_terminal("t1");
    hub.terminal("/w", Some("ssh:host"), "t1", Some("input-1"), None);

    let complete = next(&mut stream).await;
    assert_eq!(complete["type"], "task_complete");
    assert_eq!(complete["inputId"], "input-1");
    let snapshot = next(&mut stream).await;
    assert_eq!(snapshot["type"], "task_snapshot_updated");
    assert_eq!(snapshot["workspaceKey"], "ssh:host");
    assert_eq!(next(&mut ready).await["reason"], "prompt_completed");
    let outcome = next(&mut terminal).await;
    assert_eq!(outcome["outcome"], "succeeded");
    assert_eq!(outcome["inputId"], "input-1");
}

#[tokio::test]
async fn failure_emits_task_error_and_prompt_failed() {
    let hub = Arc::new(TaskEventHub::default());
    let mut stream = hub.listen_stream("t1");
    let mut ready = hub.listen_ready("t1");
    hub.terminal("/w", None, "t1", None, Some("boom"));
    let error = next(&mut stream).await;
    assert_eq!(error["type"], "task_error");
    assert_eq!(error["error"], "boom");
    assert_eq!(next(&mut ready).await["reason"], "prompt_failed");
}

#[tokio::test]
async fn handlers_extract_task_id_from_string_or_params() {
    let hub = Arc::new(TaskEventHub::default());
    let handlers = hub.handlers();
    let stream = handlers
        .iter()
        .find(|(name, _)| *name == "onDynamicStreamEvent")
        .unwrap()
        .1
        .clone();
    let mut by_string = stream(Wire::Str("t1".to_owned())).unwrap();
    let mut by_object = stream(Wire::Object(json!({"taskId": "t1"}))).unwrap();
    hub.run_started(&target(), "input-1");
    assert_eq!(next(&mut by_string).await["type"], "task_run_started");
    assert_eq!(next(&mut by_object).await["type"], "task_run_started");
}
