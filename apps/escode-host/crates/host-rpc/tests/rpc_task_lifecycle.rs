//! `escode-task.closeTask` / `resumeTask` 端到端（真 embedded 实例 + 真 WebSocket）：
//! 建会话 → resumeTask 回快照 meta → closeTask 关会话并删索引行。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn resume_and_close_a_session_task() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    support::prepare_index(&data_base_dir);
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&serde_json::json!({
            "providerId": "fixture",
            "modelId": "fixture-model",
            "reasoningLevel": "none",
            "baseUrl": "http://127.0.0.1:9/v1",
        }))
        .unwrap(),
    )
    .unwrap();

    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        71,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "config": model_config.to_string_lossy(),
            "mode": "build",
            "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
            "persistence": "immediate",
            "sessionTraceId": "trace-lifecycle",
        }))]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 71).await else {
        panic!("snapshot object")
    };
    let session_id = snapshot["session"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();

    // resumeTask：复用 session/resume（thoughtLevel 重放）并回索引 meta。
    call(
        &mut socket,
        72,
        "escode-task",
        "resumeTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": session_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
            "thoughtLevel": "none",
        }))]),
    )
    .await;
    let Wire::Object(meta) = read_response(&mut socket, 72).await else {
        panic!("meta object")
    };
    assert!(meta["taskId"].is_string(), "resumeTask 应答: {meta}");
    assert_eq!(meta["taskId"], serde_json::json!(session_id));
    assert_eq!(meta["provider"], serde_json::json!("glm"));
    assert_eq!(meta["mode"], serde_json::json!("build"));

    // closeTask：会话关闭 + 索引行删除。
    call(
        &mut socket,
        73,
        "escode-task",
        "closeTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": session_id,
        }))]),
    )
    .await;
    assert_eq!(read_response(&mut socket, 73).await, Wire::Undefined);

    call(
        &mut socket,
        74,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": session_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        }))]),
    )
    .await;
    assert_eq!(
        read_response(&mut socket, 74).await,
        Wire::Undefined,
        "删除后的任务行不再出现在索引读面"
    );

    call(
        &mut socket,
        75,
        "escode-session",
        "listSessions",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        }))]),
    )
    .await;
    let sessions = read_response(&mut socket, 75)
        .await
        .into_object()
        .expect("listSessions payload");
    let items = sessions.as_array().expect("sessions array");
    assert!(
        items
            .iter()
            .all(|session| session["sessionId"] != serde_json::json!(session_id)),
        "closeTask 后活动会话列表不应再包含它: {sessions}"
    );

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_task_creates_a_session_and_index_row() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    support::prepare_index(&data_base_dir);
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&serde_json::json!({
            "providerId": "fixture",
            "modelId": "fixture-model",
            "reasoningLevel": "none",
            "baseUrl": "http://127.0.0.1:9/v1",
        }))
        .unwrap(),
    )
    .unwrap();

    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        81,
        "escode-task",
        "createTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "config": model_config.to_string_lossy(),
            "mode": "plan",
            "modelSelection": {
                "providerId": "fixture", "modelId": "fixture-model",
                "options": {"reasoningLevel": "none"},
            },
        }))]),
    )
    .await;
    let Wire::Object(created) = read_response(&mut socket, 81).await else {
        panic!("createTask 应答应为对象")
    };
    let task_id = created["taskId"].as_str().expect("taskId").to_owned();
    assert!(task_id.starts_with("sess_"), "{created}");
    // plan 在 session 快照里是 planEnabled + core mode build（TS `fromESCodeMode` 同口径）。
    assert_eq!(created["mode"], serde_json::json!("build"), "{created}");
    assert!(
        created["initialSlashCommands"].as_array().is_some(),
        "createTask 必须携带 initialSlashCommands: {created}"
    );

    call(
        &mut socket,
        82,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        }))]),
    )
    .await;
    let Wire::Object(meta) = read_response(&mut socket, 82).await else {
        panic!("创建的 task 必须已写入索引")
    };
    assert_eq!(meta["taskId"], serde_json::json!(task_id));

    call(
        &mut socket,
        83,
        "escode-task",
        "closeTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"taskId": task_id}))]),
    )
    .await;
    assert_eq!(read_response(&mut socket, 83).await, Wire::Undefined);

    surface.shutdown().await;
}
