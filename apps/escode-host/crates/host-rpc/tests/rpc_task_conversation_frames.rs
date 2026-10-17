//! `escode-task.onDynamicConversationFrames` 端到端（docs/specs/bots-v4-stream.md）：真实例 +
//! fixture 模型，监听先收到 snapshot，发送输入后收到同一订阅的 deltas（轮头 running → 停止后中断终态）。
mod support;

use serde_json::{Value, json};
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn conversation_frames_deliver_snapshot_then_deltas() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    let workspace_path = dir.path().join("workspace");
    for path in [&home, &data_base_dir, &workspace_path] {
        std::fs::create_dir_all(path).unwrap();
    }
    let workspace = workspace_path.to_string_lossy().into_owned();
    prepare_index(&data_base_dir);
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&json!({
            "providerId": "fixture", "modelId": "fixture-model", "reasoningLevel": "none",
            "baseUrl": "http://127.0.0.1:9/v1",
        }))
        .unwrap(),
    )
    .unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut client = ControllerClient::new_client(&surface).await;
    let created = client
        .call(
            "escode-task",
            "createTask",
            json!({
                "workspacePath": workspace, "workspaceIdentity": workspace,
                "config": model_config.to_string_lossy(), "mode": "build",
                "titleGenerationEnabled": false,
                "modelSelection": {"providerId": "fixture", "modelId": "fixture-model",
                    "options": {"reasoningLevel": "none"}},
            }),
        )
        .await
        .expect("createTask")
        .into_object()
        .unwrap();
    let task_id = created["taskId"].as_str().unwrap().to_owned();
    let target =
        json!({"taskId": task_id, "workspacePath": workspace, "workspaceIdentity": workspace});

    client.listen_id = client
        .send(
            session::REQUEST_EVENT_LISTEN,
            "escode-task",
            "onDynamicConversationFrames",
            Wire::Object(target.clone()),
        )
        .await;
    let snapshot = client
        .controller_frame(|frame| frame["payload"]["kind"] == "snapshot")
        .await;
    assert_eq!(snapshot["topic"], format!("conversation/{task_id}"));
    let subscription = snapshot["subscriptionId"].clone();

    let mut prompt = target.clone();
    prompt["traceId"] = Value::from("frames-input-1");
    prompt["content"] = Value::from("hello");
    client
        .call("escode-task", "sendPrompt", prompt)
        .await
        .expect("sendPrompt");
    // 同一订阅的 deltas 里出现本轮轮头，sourceCommandId 即 sendPrompt 的 traceId。
    let running = client
        .controller_frame(|frame| {
            frame["subscriptionId"] == subscription && has_turn(frame, "running")
        })
        .await;
    assert_eq!(running["payload"]["kind"], "deltas");
    // fixture 模型地址不可达会持续重试；停止本轮，中断终态同样经这条订阅送达。
    client
        .call("escode-task", "stopGeneration", target.clone())
        .await
        .expect("stopGeneration");
    client
        .controller_frame(|frame| {
            frame["subscriptionId"] == subscription && has_turn(frame, "completedInterrupted")
        })
        .await;
}

fn has_turn(frame: &Value, state: &str) -> bool {
    frame["payload"]["deltas"].as_array().is_some_and(|deltas| {
        deltas.iter().any(|delta| {
            delta["row"]["kind"] == "turnHeader"
                && delta["row"]["state"] == state
                && delta["row"]["sourceCommandId"] == "frames-input-1"
        })
    })
}
