//! `off-peak-task` 本地管理面端到端（真 WebSocket + 真 SQLite）。
mod support;
use serde_json::{Value, json};
use support::*;

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    call(
        socket,
        id,
        "off-peak-task",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(socket, id).await
}

async fn call_id(socket: &mut Socket, id: i64, method: &str, task_id: &str) -> Wire {
    call(
        socket,
        id,
        "off-peak-task",
        method,
        &Wire::Array(vec![Wire::Str(task_id.to_owned())]),
    )
    .await;
    read_response(socket, id).await
}

fn prepare_database(data_base_dir: &std::path::Path) {
    let path = data_base_dir.join(".escode/v2/tasks-index.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE tasks (workspace_key TEXT, task_id TEXT, title TEXT);
             CREATE TABLE off_peak_tasks (
               off_peak_task_id TEXT PRIMARY KEY, server_ticket_id TEXT,
               title TEXT NOT NULL DEFAULT '', conversation_id TEXT, session_id TEXT,
               prompt TEXT NOT NULL, permission_mode TEXT NOT NULL, model TEXT,
               thought_level TEXT, model_selection TEXT, workspace_key TEXT NOT NULL,
               workspace_path TEXT NOT NULL, workspace_identity TEXT, status TEXT NOT NULL,
               queued_at INTEGER NOT NULL, started_at INTEGER, ended_at INTEGER,
               failure_reason TEXT, files_changed INTEGER, settled_at INTEGER,
               history_deleted_at INTEGER, registered_at INTEGER,
               schedulable INTEGER NOT NULL DEFAULT 0, queue_position INTEGER,
               next_poll_at INTEGER, claim_running INTEGER NOT NULL DEFAULT 0,
               claimed_at INTEGER, attempt_count INTEGER NOT NULL DEFAULT 0,
               last_error TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
             );
             INSERT INTO off_peak_tasks (
               off_peak_task_id, title, prompt, permission_mode, model_selection,
               workspace_key, workspace_path, status, queued_at, schedulable,
               created_at, updated_at
             ) VALUES (
               'offpeak-1', 'Queued', 'do work', 'default',
               '{\"providerId\":\"p\",\"modelId\":\"m\"}', '/w', '/w', 'queued', 1, 1, 1, 1
             );",
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn off_peak_management_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    prepare_database(&data_base_dir);
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    let listed = call_json(&mut socket, 1, "list", json!({})).await;
    let listed = listed.into_object().expect("list object");
    let tasks = listed.as_array().expect("list array");
    assert_eq!(tasks.len(), 1);

    let got = call_id(&mut socket, 2, "get", "offpeak-1").await;
    let Wire::Object(task) = got else {
        panic!("get object");
    };
    assert_eq!(task["status"], json!("queued"));
    assert_eq!(task["modelSelection"]["modelId"], json!("m"));

    call(
        &mut socket,
        3,
        "off-peak-task",
        "updateTask",
        &Wire::Array(vec![
            Wire::Str("offpeak-1".to_owned()),
            Wire::Object(json!({"title": "Renamed", "prompt": "new"})),
        ]),
    )
    .await;
    let Wire::Object(updated) = read_response(&mut socket, 3).await else {
        panic!("update object");
    };
    assert_eq!(updated["title"], json!("Renamed"));

    let paused = call_id(&mut socket, 4, "pauseTask", "offpeak-1").await;
    let Wire::Object(paused) = paused else {
        panic!("pause object");
    };
    assert_eq!(paused["status"], json!("paused"));
    let resumed = call_id(&mut socket, 5, "continueTask", "offpeak-1").await;
    let Wire::Object(resumed) = resumed else {
        panic!("continue object");
    };
    assert_eq!(resumed["status"], json!("queued"));

    let cancelled = call_id(&mut socket, 6, "cancelTask", "offpeak-1").await;
    let Wire::Object(cancelled) = cancelled else {
        panic!("cancel object");
    };
    assert_eq!(cancelled["status"], json!("cancelled"));

    call_id(&mut socket, 7, "deleteTask", "offpeak-1").await;
    assert_eq!(
        call_id(&mut socket, 8, "get", "offpeak-1").await,
        Wire::Object(Value::Null)
    );

    // 未迁移的创建/额度成员：显式 NotImplemented，不挂起。
    let gap = call_json(
        &mut socket,
        9,
        "createTask",
        json!({"title": "x", "prompt": "y", "permissionMode": "default", "modelSelection": {"providerId": "p", "modelId": "m"}, "workspacePath": "/w"}),
    )
    .await
    .into_object()
    .expect("gap object");
    assert_eq!(gap["name"], json!("NotImplemented"));
}
