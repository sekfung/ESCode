//! `escode-task` 输入/停止/交互端到端（真 embedded 实例 + 真 WebSocket）。
mod support;
use support::*;

async fn create_fixture() -> (tempfile::TempDir, RpcSurface, Socket, serde_json::Value) {
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
        91,
        "escode-task",
        "createTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "config": model_config.to_string_lossy(),
            "mode": "build",
            "modelSelection": {
                "providerId": "fixture", "modelId": "fixture-model",
                "options": {"reasoningLevel": "none"},
            },
        }))]),
    )
    .await;
    let Wire::Object(created) = read_response(&mut socket, 91).await else {
        panic!("createTask 应答应为对象")
    };
    (dir, surface, socket, created)
}

#[tokio::test(flavor = "multi_thread")]
async fn send_stop_compact_and_interaction_commands_reach_the_runtime() {
    let (dir, surface, mut socket, created) = create_fixture().await;
    let task_id = created["taskId"].as_str().unwrap().to_owned();
    let workspace = dir.path().join("workspace").to_string_lossy().into_owned();

    call(
        &mut socket,
        92,
        "escode-task",
        "sendPrompt",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "traceId": "input-fixture-1",
            "content": "hello",
        }))]),
    )
    .await;
    assert_eq!(
        read_response(&mut socket, 92).await,
        Wire::Undefined,
        "sendPrompt 的 V4 ACK 应被接受"
    );

    call(
        &mut socket,
        93,
        "escode-task",
        "stopGeneration",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        }))]),
    )
    .await;
    let stopped = read_response(&mut socket, 93).await;
    // 回合可能已经因连接失败结束：stop 仍是 accepted/noop 之一（不得是 NotImplemented）。
    assert_eq!(stopped, Wire::Undefined, "stopGeneration 应完成");

    call(
        &mut socket,
        94,
        "escode-task",
        "respondPermission",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "requestId": "nope",
            "optionId": "allow",
        }))]),
    )
    .await;
    // 未知 interaction 的 ACK 是 noop（TS `assertV4CommandAckOk` 同口径）→ true。
    let answered = read_response(&mut socket, 94)
        .await
        .into_object()
        .expect("permission 载荷");
    assert_eq!(answered, serde_json::json!(true), "permission 应到达运行时");

    call(
        &mut socket,
        95,
        "escode-task",
        "respondElicitation",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "requestId": "nope",
            "action": "accept",
        }))]),
    )
    .await;
    let answered = read_response(&mut socket, 95)
        .await
        .into_object()
        .expect("elicitation 载荷");
    assert_eq!(
        answered,
        serde_json::json!(true),
        "elicitation 应到达运行时"
    );

    call(
        &mut socket,
        96,
        "escode-task",
        "compactSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "instructions": "keep short",
        }))]),
    )
    .await;
    let compacted = read_response(&mut socket, 96)
        .await
        .into_object()
        .expect("compact 结果");
    assert_eq!(
        compacted["compact"]["state"],
        serde_json::json!("accepted"),
        "{compacted}"
    );
    assert_eq!(
        compacted["snapshot"]["session"]["sessionId"],
        serde_json::json!(task_id)
    );

    call(
        &mut socket,
        97,
        "escode-task",
        "closeTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"taskId": task_id}))]),
    )
    .await;
    assert_eq!(read_response(&mut socket, 97).await, Wire::Undefined);
    surface.shutdown().await;
}
