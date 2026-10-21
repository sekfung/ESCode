//! automation 本地管理面端到端（真 WebSocket + 真 SQLite）。
mod support;
use serde_json::{Value, json};
use support::*;

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    call(
        socket,
        id,
        "escode-agent",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
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
            "CREATE TABLE automations (
               automation_id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
               cron_expr TEXT NOT NULL, prompt TEXT NOT NULL, model TEXT, provider TEXT,
               mode TEXT, thought_level TEXT, model_selection TEXT,
               workspace_key TEXT NOT NULL, workspace_path TEXT NOT NULL, workspace_identity TEXT,
               target_task_id TEXT, bot_delivery_target TEXT,
               location_kind TEXT NOT NULL DEFAULT 'local', recurring INTEGER NOT NULL DEFAULT 1,
               max_runs INTEGER, end_at INTEGER, schedule_rule TEXT,
               schedule_edited_by_user INTEGER NOT NULL DEFAULT 0,
               run_count INTEGER NOT NULL DEFAULT 0,
               scheduled_run_count INTEGER NOT NULL DEFAULT 0,
               enabled INTEGER NOT NULL DEFAULT 1,
               lifecycle_status TEXT NOT NULL DEFAULT 'active', next_run_at INTEGER,
               last_run_at INTEGER, running INTEGER NOT NULL DEFAULT 0, claimed_at INTEGER,
               dispatch_status TEXT NOT NULL DEFAULT 'idle',
               dispatch_attempts INTEGER NOT NULL DEFAULT 0, retry_at INTEGER, last_error TEXT,
               created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
             );
             CREATE TABLE automation_runs (
               run_id TEXT PRIMARY KEY, automation_id TEXT NOT NULL, workspace_key TEXT NOT NULL,
               scheduled_at INTEGER, trigger TEXT NOT NULL DEFAULT 'schedule',
               model_selection TEXT, dispatch_status TEXT NOT NULL DEFAULT 'claimed',
               outcome TEXT, session_id TEXT, error TEXT,
               attempts INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             INSERT INTO automations (
               automation_id, title, cron_expr, prompt, workspace_key, workspace_path,
               enabled, lifecycle_status, created_at, updated_at
             ) VALUES ('automation-1', 'Daily', '0 9 * * *', 'do', '/w', '/w', 1, 'active', 1, 1);
             INSERT INTO automation_runs (
               run_id, automation_id, workspace_key, trigger, created_at, updated_at
             ) VALUES ('automation-1:manual:x', 'automation-1', '/w', 'manual', 1, 1);",
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn automation_management_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    prepare_database(&data_base_dir);
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    let listed = call_json(
        &mut socket,
        1,
        "listAutomations",
        json!({"workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await
    .into_object()
    .expect("list");
    let automations = listed.as_array().unwrap();
    assert_eq!(automations.len(), 1);
    assert_eq!(automations[0]["automationId"], json!("automation-1"));
    assert_eq!(automations[0]["recurring"], json!(true));
    assert_eq!(automations[0]["enabled"], json!(true));

    let all = call_json(&mut socket, 2, "listAllAutomations", json!({}))
        .await
        .into_object()
        .expect("all");
    assert_eq!(all.as_array().unwrap().len(), 1);

    call_json(
        &mut socket,
        3,
        "setAutomationEnabled",
        json!({"automationId": "automation-1", "enabled": false, "workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await;
    let after = call_json(
        &mut socket,
        4,
        "listAutomations",
        json!({"workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await
    .into_object()
    .expect("after");
    assert_eq!(after[0]["enabled"], json!(false));
    assert_eq!(after[0]["lifecycleStatus"], json!("paused"));

    let runs = call_json(
        &mut socket,
        5,
        "listAutomationRuns",
        json!({"automationId": "automation-1", "workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await
    .into_object()
    .expect("runs");
    assert_eq!(runs.as_array().unwrap().len(), 1);

    call_json(
        &mut socket,
        6,
        "deleteAutomationRun",
        json!({"runId": "automation-1:manual:x", "workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await;
    let runs = call_json(
        &mut socket,
        7,
        "listAutomationRuns",
        json!({"automationId": "automation-1", "workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await
    .into_object()
    .expect("runs after");
    assert_eq!(runs.as_array().unwrap().len(), 0);

    call_json(
        &mut socket,
        8,
        "deleteAutomation",
        json!({"automationId": "automation-1", "workspacePath": "/w", "workspaceIdentity": "/w"}),
    )
    .await;
    let all = call_json(&mut socket, 9, "listAllAutomations", json!({}))
        .await
        .into_object()
        .expect("all after");
    assert_eq!(all.as_array().unwrap().len(), 0);

    // create：相对延迟由服务端真实时钟锚定，固定只运行一次。
    let created = call_json(
        &mut socket,
        10,
        "createAutomation",
        json!({
            "title": "x", "cronExpr": "0 9 * * *", "prompt": "y",
            "relativeDelayMinutes": 5, "recurring": false,
            "workspacePath": "/w", "workspaceIdentity": "/w"
        }),
    )
    .await
    .into_object()
    .expect("created");
    let created_id = created["automationId"]
        .as_str()
        .expect("automationId")
        .to_owned();
    assert_eq!(created["recurring"], json!(false));
    assert!(created["nextRunAt"].as_i64().is_some());

    // update：只改标题不重算 nextRunAt。
    let updated = call_json(
        &mut socket,
        11,
        "updateAutomation",
        json!({
            "automationId": created_id,
            "title": "renamed",
            "workspacePath": "/w", "workspaceIdentity": "/w"
        }),
    )
    .await
    .into_object()
    .expect("updated");
    assert_eq!(updated["title"], json!("renamed"));
    assert_eq!(updated["nextRunAt"], created["nextRunAt"]);
}
