//! `bots` 配置/状态/绑定子集端到端（真 WebSocket + 真 JSON 文件）。
mod support;
use serde_json::{Value, json};
use support::*;

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    call(
        socket,
        id,
        "bots",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(socket, id).await
}

async fn call_id(socket: &mut Socket, id: i64, method: &str, value: &str) -> Wire {
    call(
        socket,
        id,
        "bots",
        method,
        &Wire::Array(vec![Wire::Str(value.to_owned())]),
    )
    .await;
    read_response(socket, id).await
}

fn prepare_files(data_base_dir: &std::path::Path, home: &std::path::Path) {
    let v2 = data_base_dir.join(".escode/v2");
    std::fs::create_dir_all(&v2).unwrap();
    std::fs::write(
        v2.join("bot-state.v3.json"),
        serde_json::to_string(&json!({
            "version": 3,
            "bots": {
                "bot-1": {
                    "botId": "bot-1", "workspacePath": "/w", "mode": "draft",
                    "activeTaskId": null, "telegramOffset": 42, "updatedAt": 1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let settings = home.join(".escode/v2");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(
        settings.join("setting.json"),
        serde_json::to_string(&json!({
            "lastWorkspaceSession": [
                {"workspacePath": "/recent", "kind": "local"},
                {"workspacePath": "/remote", "kind": "remote", "workspaceIdentity": "remote:id"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bots_config_state_and_bind_codes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    prepare_files(&data_base_dir, &home);
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    let config = call_json(&mut socket, 1, "getConfig", json!({}))
        .await
        .into_object()
        .expect("config");
    assert_eq!(config, json!({"version": 3, "bots": []}));

    let bot = json!({
        "id": "bot-1",
        "name": "  Bot  ",
        "provider": "telegram",
        "enabled": true,
        "allowedWorkspaces": ["*"],
        "allowedCommands": {"status": true, "new": true, "workspace": true, "model": true, "mode": true, "thoughtLevel": true, "reply": true},
        "currentOptions": {},
        "replyMode": "assistant_changes"
    });
    let saved = call_json(
        &mut socket,
        2,
        "saveBot",
        json!({"bot": bot, "credentialValue": "token"}),
    )
    .await
    .into_object()
    .expect("saveBot");
    assert_eq!(saved["id"], json!("bot-1"));
    assert_eq!(saved["name"], json!("Bot"));
    assert_eq!(saved["credentialRef"], json!("bot:bot-1:credential"));

    let status = call_json(&mut socket, 3, "getStatus", json!({}))
        .await
        .into_object()
        .expect("status");
    assert_eq!(status["botsCount"], json!(1));
    assert_eq!(status["contextsCount"], json!(1));
    assert_eq!(status["botRuntime"][0]["status"], json!("idle"));
    assert_eq!(status["botRuntime"][0]["offset"], json!(42));

    let states = call_json(&mut socket, 4, "getBotStates", json!({}))
        .await
        .into_object()
        .expect("states");
    assert_eq!(states.as_array().unwrap().len(), 1);

    let refs = call_json(
        &mut socket,
        5,
        "listWorkspaceRefs",
        json!({"currentWorkspace": {"workspacePath": "/current", "workspaceIdentity": "id:current"}}),
    )
    .await
    .into_object()
    .expect("refs");
    let refs = refs.as_array().unwrap();
    assert_eq!(refs.len(), 3);
    assert!(
        refs.iter()
            .any(|item| item["workspacePath"] == json!("/remote"))
    );

    let bind = call_json(
        &mut socket,
        6,
        "createBindCode",
        json!({"botId": "bot-1", "ttlMs": 1000}),
    )
    .await
    .into_object()
    .expect("bind");
    assert_eq!(bind["code"].as_str().unwrap().len(), 6);

    // resetBotState 经 ProxyChannel 是字符串实参。
    call_id(&mut socket, 8, "resetBotState", "bot-1").await;
    let states = call_json(&mut socket, 9, "getBotStates", json!({}))
        .await
        .into_object()
        .expect("states after");
    assert_eq!(states.as_array().unwrap().len(), 0);

    let stripped = call_id(&mut socket, 10, "removeBotSecret", "bot-1").await;
    let Wire::Object(stripped) = stripped else {
        panic!("removeBotSecret object");
    };
    assert!(stripped.get("credentialRef").is_none());

    call_id(&mut socket, 11, "deleteBot", "bot-1").await;
    let listed = call_json(&mut socket, 12, "listBots", json!({}))
        .await
        .into_object()
        .expect("list");
    assert_eq!(listed, json!([]));

    call_json(&mut socket, 13, "syncAppRuntimePreferences", json!({})).await;

    let gap = call_json(&mut socket, 14, "beginFeishuRegistration", json!({}))
        .await
        .into_object()
        .expect("gap");
    assert_eq!(gap["name"], json!("NotImplemented"));
}
