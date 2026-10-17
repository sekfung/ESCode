//! `escode-task` 端到端夹具：真实 Host surface + 已建 tasks-index 的临时 workspace。
//! 从 `rpc_task_remaining.rs` 拆出，多个 task 测试共用同一套启动与调用路径。

use super::{RpcSurface, Socket, Wire, call, open, read_response, start_with_data_base_dir};

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub surface: RpcSurface,
    pub socket: Socket,
    pub task_id: String,
    pub workspace: String,
}

pub async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    super::prepare_index(&data_base_dir);
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
        dir,
        surface,
        socket,
        task_id,
        workspace,
    }
}

pub fn object(wire: Wire) -> serde_json::Value {
    wire.into_object().expect("object response")
}

pub async fn call_task(
    fixture: &mut Fixture,
    id: i64,
    method: &str,
    extra: serde_json::Value,
) -> Wire {
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
