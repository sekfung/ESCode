//! `subagents` 端到端（真 WebSocket，`rust-u2-subagents.md` §4）。
//!
//! 覆盖 8/8：`list` + 目录解析 + 状态写入面 + 写三法（create/update/delete）。
mod support;
use support::*;

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: serde_json::Value) -> Wire {
    call(
        socket,
        id,
        "subagents",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(socket, id).await
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_and_state_writes_then_list_reports_an_explicit_gap() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    let agents_root = home.join(".escode").join("agents");
    let state_path = home.join(".escode").join("v2").join("agents-state.json");
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    // ── 目录解析 ────────────────────────────────────────────────────
    let answer = call_json(
        &mut socket,
        1,
        "getPrimaryUserAgentsDirectory",
        serde_json::json!({"provider": "glm"}),
    )
    .await;
    let Wire::Object(answer) = answer else {
        panic!("应答应为对象")
    };
    assert_eq!(
        answer["path"],
        serde_json::json!(agents_root.to_string_lossy())
    );
    assert!(agents_root.is_dir(), "目录必须真的落盘");

    // ── 禁用集：去重后升序 ──────────────────────────────────────────
    for (id, agent) in [(2, "b"), (3, "a"), (4, "b")] {
        call_json(
            &mut socket,
            id,
            "setEnabled",
            serde_json::json!({"agentId": agent, "enabled": false}),
        )
        .await;
    }
    let state = read_state(&state_path);
    assert_eq!(
        state["disabledAgentIds"],
        serde_json::json!(["a", "b"]),
        "{state}"
    );
    call_json(
        &mut socket,
        5,
        "setEnabled",
        serde_json::json!({"agentId": "a", "enabled": true}),
    )
    .await;
    assert_eq!(
        read_state(&state_path)["disabledAgentIds"],
        serde_json::json!(["b"])
    );

    // ── 内置模型覆盖：写入与清除 ────────────────────────────────────
    call_json(
        &mut socket,
        6,
        "setBuiltInModelOverride",
        serde_json::json!({
            "agentName": "general-purpose",
            "modelSelection": {"providerId": " glm ", "modelId": "glm-5", "options": {"reasoningLevel": "high"}}
        }),
    )
    .await;
    let state = read_state(&state_path);
    assert_eq!(
        state["builtInModelSelectionOverrides"]["general-purpose"],
        serde_json::json!({
            "providerId": "glm", "modelId": "glm-5", "options": {"reasoningLevel": "high"}
        }),
        "写入前要 trim：{state}"
    );
    call_json(
        &mut socket,
        7,
        "setBuiltInModelOverride",
        serde_json::json!({"agentName": "general-purpose", "modelSelection": null}),
    )
    .await;
    assert_eq!(
        read_state(&state_path)["builtInModelSelectionOverrides"],
        serde_json::json!({}),
        "null 表示清除"
    );

    // ── 插件覆盖：id 校验 ──────────────────────────────────────────
    let error = call_json(
        &mut socket,
        8,
        "setPluginAgentModelOverride",
        serde_json::json!({"agentId": "demo", "modelSelection": null}),
    )
    .await
    .into_object()
    .expect("错误载荷");
    assert_eq!(
        error["message"],
        serde_json::json!("无效插件 Subagent 身份"),
        "{error}"
    );

    // ── `list`：内置两个 agent + 用户目录里的自定义 agent ─────────────
    let agents_root = home.join(".escode").join("agents");
    std::fs::create_dir_all(&agents_root).unwrap();
    std::fs::write(
        agents_root.join("reviewer.md"),
        "---\nname: reviewer\ndescription: 代码审查\n---\n你是审查员。",
    )
    .unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let answer = call_json(
        &mut socket,
        20,
        "list",
        serde_json::json!({"workspacePath": workspace.to_string_lossy()}),
    )
    .await;
    let Wire::Object(answer) = answer else {
        panic!("list 应答应为对象")
    };
    assert_eq!(
        answer["capability"]["userScopeAvailable"],
        serde_json::json!(true),
        "{answer}"
    );
    let names: Vec<&str> = answer["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"general-purpose"), "{names:?}");
    assert!(names.contains(&"Explore"), "{names:?}");
    assert!(names.contains(&"reviewer"), "{names:?}");
    // 禁用的 user agent 在列表里显示为 enabled=false。
    let disabled_id = answer["userAgents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == serde_json::json!("reviewer"))
        .map(|agent| agent["id"].as_str().unwrap().to_owned())
        .expect("reviewer 在 userAgents 里");
    call_json(
        &mut socket,
        21,
        "setEnabled",
        serde_json::json!({"agentId": disabled_id, "enabled": false}),
    )
    .await;
    let answer = call_json(
        &mut socket,
        22,
        "list",
        serde_json::json!({"workspacePath": workspace.to_string_lossy()}),
    )
    .await;
    let Wire::Object(answer) = answer else {
        panic!("list 应答应为对象")
    };
    let reviewer = answer["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == serde_json::json!("reviewer"))
        .expect("reviewer 仍在列表里");
    assert_eq!(reviewer["enabled"], serde_json::json!(false));

    // ── 写三法：创建 → 改名（禁用态跟随）→ 删除 ────────────────────
    let answer = call_json(
        &mut socket,
        40,
        "createAgent",
        serde_json::json!({
            "config": {"name": "auditor", "description": "审计", "systemPrompt": "你是审计员。"}
        }),
    )
    .await;
    let Wire::Object(answer) = answer else {
        panic!("createAgent 应答应为对象")
    };
    let created_id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let created_path = answer["agent"]["path"].as_str().unwrap().to_owned();
    assert_eq!(created_id, "user:user:auditor");
    assert!(std::path::Path::new(&created_path).exists());

    // 同名再建 → 明确报错。
    let error = call_json(
        &mut socket,
        41,
        "createAgent",
        serde_json::json!({
            "config": {"name": "auditor", "description": "d", "systemPrompt": "p"}
        }),
    )
    .await
    .into_object()
    .expect("错误载荷");
    assert_eq!(
        error["message"],
        serde_json::json!("Agent file \"auditor.md\" already exists"),
        "{error}"
    );

    // 禁用后改名：新 id 继承禁用态，旧文件删除。
    call_json(
        &mut socket,
        42,
        "setEnabled",
        serde_json::json!({"agentId": created_id, "enabled": false}),
    )
    .await;
    let answer = call_json(
        &mut socket,
        43,
        "updateAgent",
        serde_json::json!({
            "agentId": created_id,
            "oldFilePath": created_path,
            "config": {"name": "auditor-two", "description": "审计", "systemPrompt": "你是审计员。"}
        }),
    )
    .await;
    let Wire::Object(answer) = answer else {
        panic!("updateAgent 应答应为对象")
    };
    let renamed_id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let renamed_path = answer["agent"]["path"].as_str().unwrap().to_owned();
    assert_eq!(renamed_id, "user:user:auditor-two");
    assert!(
        !std::path::Path::new(&created_path).exists(),
        "旧文件应删除"
    );
    let list_after_rename = call_json(
        &mut socket,
        44,
        "list",
        serde_json::json!({"workspacePath": workspace.to_string_lossy()}),
    )
    .await;
    let Wire::Object(list_after_rename) = list_after_rename else {
        panic!("list 应答应为对象")
    };
    let renamed = list_after_rename["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == serde_json::json!("auditor-two"))
        .expect("改名后应出现在列表里");
    assert_eq!(
        renamed["enabled"],
        serde_json::json!(false),
        "禁用态跟随改名"
    );

    // 删除后不再出现。
    call_json(
        &mut socket,
        45,
        "deleteAgent",
        serde_json::json!({"filePath": renamed_path, "agentId": renamed_id}),
    )
    .await;
    assert!(!std::path::Path::new(&renamed_path).exists());
    let list_after_delete = call_json(
        &mut socket,
        46,
        "list",
        serde_json::json!({"workspacePath": workspace.to_string_lossy()}),
    )
    .await;
    let Wire::Object(list_after_delete) = list_after_delete else {
        panic!("list 应答应为对象")
    };
    assert!(
        !list_after_delete["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["name"] == serde_json::json!("auditor-two")),
        "删除后不应再出现"
    );

    surface.shutdown().await;
}

fn read_state(path: &std::path::Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".to_owned());
    serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
}
