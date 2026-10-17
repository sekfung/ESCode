//! `escode-task` 命令队列端到端（S8）：owner 闸门、入队/提升/取消，
//! 以及 `syncTaskSessionBinding` 的 legacy 桩。
//!
//! 模型端点是一个只接受连接、不回响应的本地 TCP server：首轮保持 running，
//! host 的 active 表不会被终态清掉，队列语义因此可确定性断言。
//! 该端点会让运行时的读路径一起挂起，所以本用例只走 Host 内存面（入队/提升/取消）；
//! 快照 `pendingCommands` 的投影由 `task_commands` 单测覆盖。
mod support;
use support::*;

struct Fixture {
    _dir: tempfile::TempDir,
    surface: RpcSurface,
    socket: Socket,
    task_id: String,
    workspace: String,
    server: tokio::task::JoinHandle<()>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    support::prepare_index(&data_base_dir);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            match listener.accept().await {
                Ok((stream, _)) => held.push(stream),
                Err(_) => return,
            }
        }
    });
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&serde_json::json!({
            "providerId": "fixture",
            "modelId": "fixture-model",
            "reasoningLevel": "none",
            "baseUrl": base_url,
        }))
        .unwrap(),
    )
    .unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;
    call(
        &mut socket,
        1,
        "escode-task",
        "createTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "config": model_config.to_string_lossy(),
            "mode": "build",
            "titleGenerationEnabled": false,
            "modelSelection": {
                "providerId": "fixture", "modelId": "fixture-model",
                "options": {"reasoningLevel": "none"},
            },
        }))]),
    )
    .await;
    let Wire::Object(created) = read_response(&mut socket, 1).await else {
        panic!("createTask 应答应为对象")
    };
    let task_id = created["taskId"].as_str().unwrap().to_owned();
    Fixture {
        _dir: dir,
        surface,
        socket,
        task_id,
        workspace,
        server,
    }
}

fn object(wire: Wire) -> serde_json::Value {
    wire.into_object().expect("object response")
}

async fn task_call(fixture: &mut Fixture, id: i64, method: &str, extra: serde_json::Value) -> Wire {
    let mut params = serde_json::json!({
        "taskId": fixture.task_id,
        "workspacePath": fixture.workspace,
        "workspaceIdentity": fixture.workspace,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        params[key] = value;
    }
    call(
        &mut fixture.socket,
        id,
        "escode-task",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(&mut fixture.socket, id).await
}

async fn pending_commands(fixture: &mut Fixture, id: i64) -> Vec<serde_json::Value> {
    let snapshot = object(task_call(fixture, id, "getTaskSnapshot", serde_json::json!({})).await);
    snapshot["runtime"]["pendingCommands"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("pendingCommands 应为数组: {snapshot}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn command_queue_owner_promote_cancel_and_legacy_stub() {
    let mut fixture = fixture().await;

    // 没有 host 记录的 active 轮次：owner 命令被拒绝（不得写入队列）。
    let denied = object(
        task_call(
            &mut fixture,
            10,
            "enqueueTaskCommand",
            serde_json::json!({
                "commandId": "cmd-denied",
                "traceId": "input-denied",
                "type": "send_prompt",
                "content": "denied",
                "ownerRunId": "run-missing",
            }),
        )
        .await,
    );
    assert_eq!(denied["name"], "Error");
    assert_eq!(denied["message"], "No active task owner.");
    assert_eq!(denied["code"], "NO_ACTIVE_TASK_OWNER");

    // 首轮打到悬挂端点：V4 ACK 后 host 记录 active，队列不再 drain。
    task_call(
        &mut fixture,
        11,
        "sendPrompt",
        serde_json::json!({"traceId": "input-hang", "content": "first"}),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let first = object(
        task_call(
            &mut fixture,
            12,
            "enqueueTaskCommand",
            serde_json::json!({
                "commandId": "cmd-1",
                "traceId": "input-2",
                "queryId": "query-2",
                "type": "send_prompt",
                "content": "second",
                "clientId": "phone-1",
                "clientLabel": "手机",
                "ownerRunId": "input-hang",
            }),
        )
        .await,
    );
    assert_eq!(first["accepted"], true);
    assert_eq!(first["command"]["commandId"], "cmd-1");
    assert_eq!(first["command"]["workspaceKey"], fixture.workspace);
    assert_eq!(first["command"]["status"], "accepted");
    assert_eq!(first["command"]["clientId"], "phone-1");
    assert_eq!(first["command"]["clientLabel"], "手机");
    // active 轮次未终态：drain 不发送，命令保留在快照投影里。
    let pending = pending_commands(&mut fixture, 13).await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["commandId"], "cmd-1");

    let second = object(
        task_call(
            &mut fixture,
            14,
            "enqueueTaskCommand",
            serde_json::json!({
                "commandId": "cmd-2",
                "traceId": "input-3",
                "type": "send_prompt",
                "content": "third",
                "ownerRunId": "input-hang",
            }),
        )
        .await,
    );
    assert_eq!(second["accepted"], true);

    // owner 不匹配：stale 防护。
    let stale = object(
        task_call(
            &mut fixture,
            15,
            "enqueueTaskCommand",
            serde_json::json!({
                "commandId": "cmd-stale",
                "traceId": "input-stale",
                "type": "send_prompt",
                "content": "stale",
                "ownerRunId": "input-other",
            }),
        )
        .await,
    );
    assert_eq!(stale["message"], "Stale task owner command.");
    assert_eq!(stale["code"], "STALE_TASK_OWNER_COMMAND");

    let promoted = object(
        task_call(
            &mut fixture,
            16,
            "promoteTaskCommand",
            serde_json::json!({"commandId": "cmd-2", "ownerRunId": "input-hang"}),
        )
        .await,
    );
    assert_eq!(promoted["accepted"], true);
    assert_eq!(promoted["command"]["status"], "accepted");
    let pending = pending_commands(&mut fixture, 17).await;
    assert_eq!(pending[0]["commandId"], "cmd-2");
    assert_eq!(pending[1]["commandId"], "cmd-1");

    let canceled = object(
        task_call(
            &mut fixture,
            18,
            "cancelTaskCommand",
            serde_json::json!({"commandId": "cmd-2", "ownerRunId": "input-hang"}),
        )
        .await,
    );
    assert_eq!(
        canceled,
        serde_json::json!({"canceled": true, "commandId": "cmd-2", "status": "accepted"})
    );
    let canceled = object(
        task_call(
            &mut fixture,
            19,
            "cancelTaskCommand",
            serde_json::json!({"commandId": "cmd-2", "ownerRunId": "input-hang"}),
        )
        .await,
    );
    assert_eq!(canceled["reason"], "not_found");
    assert_eq!(canceled["canceled"], true);
    let pending = pending_commands(&mut fixture, 22).await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["commandId"], "cmd-1");

    let missing = object(
        task_call(
            &mut fixture,
            20,
            "promoteTaskCommand",
            serde_json::json!({"commandId": "cmd-missing", "ownerRunId": "input-hang"}),
        )
        .await,
    );
    assert_eq!(missing["message"], "Task command not found.");
    assert_eq!(missing["code"], "OWNER_COMMAND_FAILED");

    let canceled = object(
        task_call(
            &mut fixture,
            21,
            "cancelTaskCommand",
            serde_json::json!({"commandId": "cmd-1", "ownerRunId": "input-hang"}),
        )
        .await,
    );
    assert_eq!(canceled["canceled"], true);
    assert!(pending_commands(&mut fixture, 24).await.is_empty());

    let binding = object(
        task_call(
            &mut fixture,
            23,
            "syncTaskSessionBinding",
            serde_json::json!({"provider": "claude"}),
        )
        .await,
    );
    assert_eq!(
        binding["code"],
        "ESCODE_AGENT_UNSUPPORTED_LEGACY_TASK_METHOD"
    );
    assert_eq!(
        binding["message"],
        "ESCode task service adapter does not support IESCodeTaskService.syncTaskSessionBinding yet."
    );

    // 运行时读路径仍被悬挂端点占用：直接收口 surface（不 closeTask）。
    fixture.server.abort();
    fixture.surface.shutdown().await;
}
