//! off-peak 管理面单测：读取投影与状态机守卫。

use serde_json::{Value, json};

use super::OffPeakService;

fn database() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let data_base_dir = dir.path().join("data");
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
               off_peak_task_id, title, session_id, prompt, permission_mode,
               model_selection, workspace_key, workspace_path, status, queued_at,
               schedulable, created_at, updated_at
             ) VALUES (
               'offpeak-1', 'Queued', 'sess-1', 'do work', 'default',
               '{\"providerId\":\"p\",\"modelId\":\"m\"}', '/w', '/w', 'queued', 1,
               1, 1, 1
             );
             INSERT INTO off_peak_tasks (
               off_peak_task_id, title, prompt, permission_mode, workspace_key,
               workspace_path, status, queued_at, created_at, updated_at
             ) VALUES ('offpeak-2', 'Legacy', 'x', 'default', '/w', '/w', 'queued', 2, 2, 2);
             INSERT INTO tasks VALUES ('/w', 'sess-1', 'Bound session');",
        )
        .unwrap();
    (dir, data_base_dir)
}

#[tokio::test]
async fn list_and_get_project_session_title_and_selection_issue() {
    let (_dir, data_base_dir) = database();
    let service = OffPeakService::new(data_base_dir);
    let list = service.list().await.unwrap();
    let tasks = list.as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["offPeakTaskId"], json!("offpeak-2"));
    assert_eq!(tasks[0]["schedulable"], json!(false));
    assert_eq!(
        tasks[0]["modelSelectionIssue"]["code"],
        json!("repair-required")
    );
    let bound = tasks
        .iter()
        .find(|task| task["offPeakTaskId"] == json!("offpeak-1"))
        .unwrap();
    assert_eq!(bound["sessionTitle"], json!("Bound session"));
    let first = service.get("offpeak-1".to_owned()).await.unwrap();
    assert!(
        first.get("sessionTitle").is_none(),
        "get 不联查标题（TS 同）"
    );
    assert_eq!(first["modelSelection"]["modelId"], json!("m"));
    assert_eq!(first["schedulable"], json!(true));
}

#[tokio::test]
async fn editable_state_machine_and_lifecycle() {
    let (_dir, data_base_dir) = database();
    let service = OffPeakService::new(data_base_dir);
    let updated = service
        .update(
            "offpeak-1".to_owned(),
            json!({"title": "Renamed", "prompt": "new prompt", "modelSelection": {"providerId": "p", "modelId": "m2"}}),
        )
        .await
        .unwrap();
    assert_eq!(updated["title"], json!("Renamed"));
    assert_eq!(updated["modelSelection"]["modelId"], json!("m2"));

    let paused = service.pause("offpeak-1".to_owned()).await.unwrap();
    assert_eq!(paused["status"], json!("paused"));
    let resumed = service.resume("offpeak-1".to_owned()).await.unwrap();
    assert_eq!(resumed["status"], json!("queued"));
    // 非 paused 再 resume 是 null。
    assert_eq!(
        service.resume("offpeak-1".to_owned()).await.unwrap(),
        Value::Null
    );

    let cancelled = service.cancel("offpeak-1".to_owned()).await.unwrap();
    assert_eq!(cancelled["status"], json!("cancelled"));
    // 终态不可逆出：重复取消返回原行。
    let again = service.cancel("offpeak-1".to_owned()).await.unwrap();
    assert_eq!(again["status"], json!("cancelled"));
    // 终态不可编辑。
    assert_eq!(
        service
            .update("offpeak-1".to_owned(), json!({"title": "x"}))
            .await
            .unwrap(),
        Value::Null
    );
}

#[tokio::test]
async fn delete_history_requires_started_and_delete_cancels_non_terminal() {
    let (_dir, data_base_dir) = database();
    let service = OffPeakService::new(data_base_dir);
    // started_at 为空：幂等返回原行。
    let unchanged = service
        .delete_history("offpeak-1".to_owned())
        .await
        .unwrap();
    assert!(unchanged.get("historyDeletedAt").is_none());

    service.delete("offpeak-2".to_owned()).await.unwrap();
    assert_eq!(
        service.get("offpeak-2".to_owned()).await.unwrap(),
        Value::Null
    );
    let remaining = service.list().await.unwrap();
    assert_eq!(remaining.as_array().unwrap().len(), 1);
}
