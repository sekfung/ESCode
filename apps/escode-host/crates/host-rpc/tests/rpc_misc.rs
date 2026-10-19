//! 零散成员端到端：`escode-agent.listSessionSubagents`、`escode-session.getWorkspaceRuntimeIdentity`。
//!
//! 使用真 embedded runtime + 真 WebSocket；会话创建夹具与 `rpc_session_create.rs` 相同（不发模型请求）。
mod support;
use support::*;

async fn create_session(socket: &mut Socket, workspace: &str, config: &str, id: i64) -> String {
    call(
        socket,
        id,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "config": config,
            "mode": "build",
            "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
            "persistence": "immediate",
        }))]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(socket, id).await else {
        panic!("createSession answer");
    };
    snapshot["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn subagents_and_runtime_identity_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    let workspace_path = dir.path().join("workspace");
    for path in [&home, &data_base_dir, &workspace_path] {
        std::fs::create_dir_all(path).unwrap();
    }
    support::prepare_index(&data_base_dir);
    let config = dir.path().join("model.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "providerId": "fixture",
            "modelId": "fixture-model",
            "reasoningLevel": "none",
            "baseUrl": "http://127.0.0.1:9/v1",
        }))
        .unwrap(),
    )
    .unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;
    let session = create_session(&mut socket, &workspace, &config.to_string_lossy(), 1).await;

    // 运行时 identity：existing-only，返回真实实例 generation。
    call(
        &mut socket,
        2,
        "escode-session",
        "getWorkspaceRuntimeIdentity",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        }))]),
    )
    .await;
    let Wire::Object(identity) = read_response(&mut socket, 2).await else {
        panic!("identity object");
    };
    assert_eq!(identity["generation"], serde_json::json!(1));
    assert_eq!(identity["workspaceKey"], serde_json::json!(workspace));

    // 子代理列表：默认 endedLimit=20，空会话返回空结构。
    call(
        &mut socket,
        3,
        "escode-agent",
        "listSessionSubagents",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "sessionId": session,
        }))]),
    )
    .await;
    let Wire::Object(subagents) = read_response(&mut socket, 3).await else {
        panic!("subagents object");
    };
    assert_eq!(subagents["childSessionIds"], serde_json::json!([]));
    assert_eq!(subagents["running"], serde_json::json!([]));
    assert_eq!(subagents["ended"]["total"], serde_json::json!(0));
    assert_eq!(subagents["ended"]["items"], serde_json::json!([]));

    // dormant workspace：identity 查询不得启动实例。
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    call(
        &mut socket,
        4,
        "escode-session",
        "getWorkspaceRuntimeIdentity",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": other.to_string_lossy(),
            "workspaceIdentity": other.to_string_lossy(),
        }))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 4).await else {
        panic!("error object");
    };
    assert_eq!(error["name"], serde_json::json!("Error"));
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("runtime identity is unavailable"),
        "{error}"
    );
}
