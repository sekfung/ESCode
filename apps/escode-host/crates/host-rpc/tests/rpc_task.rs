//! `escode-task` channel 端到端用例：`getTaskMeta` 直读 tasks-index（真实 WebSocket + 手写 RPC 编解码）。
//! 夹具按生成资产（`host-services::tasks_index::schema::MIGRATED_DDL`）建库，与 TS Host 同物理 schema。
mod support;
use support::*;

use escode_host_services::tasks_index::schema::MIGRATED_DDL;

/// 按 TS 迁移后的物理 DDL 建一个空库并写入一行任务。
fn seed(data_base_dir: &std::path::Path, task_id: &str) -> std::path::PathBuf {
    let path = data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    for statement in MIGRATED_DDL {
        connection.execute_batch(statement).unwrap();
    }
    for (id, checksum) in escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS {
        connection
            .execute(
                "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
                rusqlite::params![id, checksum],
            )
            .unwrap();
    }
    let meta = serde_json::json!({
        "taskId": task_id,
        "traceId": format!("escode-{task_id}"),
        "title": "评审 Rust Host",
        "workspacePath": "/w",
        "createdAt": 100,
        "updatedAt": 200,
        "mode": "build",
    });
    connection
        .execute(
            "INSERT INTO tasks (
               workspace_key, workspace_path, task_id, title, mode, created_at, updated_at, meta_json
             ) VALUES ('/w', '/w', ?1, '列标题', 'build', 100, 200, ?2)",
            rusqlite::params![task_id, meta.to_string()],
        )
        .unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn get_task_meta_reads_the_shared_index() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    seed(&data_base_dir, "task_1");

    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = connect(surface.addr, "/ws", None).await;

    call(
        &mut socket,
        41,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": "/w",
            "taskId": "task_1",
        }))]),
    )
    .await;
    let meta = read_response(&mut socket, 41)
        .await
        .into_object()
        .expect("object");
    assert_eq!(meta["taskId"], "task_1");
    assert_eq!(meta["title"], "评审 Rust Host", "meta_json 优先于列");
    assert_eq!(meta["workspacePath"], "/w");
    assert_eq!(meta["createdAt"], 100);
    assert_eq!(meta["titleOverridden"], false);

    // 不存在的任务：`undefined`（TS 返回 null，Wire 层同为「无值」）。
    call(
        &mut socket,
        42,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": "/w",
            "taskId": "task_missing",
        }))]),
    )
    .await;
    assert!(matches!(
        read_response(&mut socket, 42).await,
        Wire::Undefined
    ));

    // 未迁移方法：显式缺口（列出已实现的方法）。
    call(
        &mut socket,
        43,
        "escode-task",
        "createTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({}))]),
    )
    .await;
    let error = read_response(&mut socket, 43).await;
    let Wire::Object(error) = error else {
        panic!("error object: {error:?}")
    };
    let message = error["message"].as_str().unwrap_or_default().to_owned();
    assert!(
        message.contains("does not implement `escode-task.createTask` yet"),
        "{message}"
    );
    assert!(message.contains("getTaskMeta"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_task_meta_reports_a_structured_error_when_the_index_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // dataBaseDir 存在但 TS Host 尚未建库。
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = connect(surface.addr, "/ws", None).await;

    call(
        &mut socket,
        44,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": "/w",
            "taskId": "task_1",
        }))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 44).await else {
        panic!("error object")
    };
    // RPC 错误帧按 JS Error 形状下发：结构化 code 落在 `name`。
    assert_eq!(error["name"], "TasksIndexUnavailable", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tasks-index is missing"),
        "{error}"
    );
}
