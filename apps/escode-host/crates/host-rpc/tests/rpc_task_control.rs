//! `escode-task` 任务控制与 legacy 桩端到端。
mod support;
use support::*;

async fn fixture() -> (
    tempfile::TempDir,
    RpcSurface,
    Socket,
    serde_json::Value,
    String,
) {
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
        101,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "config": model_config.to_string_lossy(),
            "mode": "build",
            "model": {
                "providerId": "fixture", "modelId": "fixture-model",
                "options": {"reasoningLevel": "none"},
            },
            "persistence": "immediate",
            "titleGenerationEnabled": false,
            "sessionTraceId": "trace-control",
        }))]),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 101).await else {
        panic!("createSession 应答应为快照对象")
    };
    let created = serde_json::json!({"taskId": snapshot["session"]["sessionId"]});
    let task_id = created["taskId"].as_str().unwrap().to_owned();
    (dir, surface, socket, created, task_id)
}

fn options_array(payload: Wire, label: &str) -> Vec<String> {
    let items = match payload {
        Wire::Array(items) => items,
        Wire::Object(serde_json::Value::Array(values)) => {
            values.into_iter().map(Wire::Object).collect()
        }
        other => panic!("{label} 应答应为数组: {other:?}"),
    };
    items
        .into_iter()
        .filter_map(Wire::into_object)
        .filter_map(|option| option["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn control_methods_update_config_and_stub_out_unsupported_paths() {
    let (dir, surface, mut socket, _created, task_id) = fixture().await;
    let workspace = dir.path().join("workspace").to_string_lossy().into_owned();
    // 新建会话可能仍在跑标题生成；V4 模式切换在运行中会被运行时拒绝（Node 同语义的 guard）。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        call(
            &mut socket,
            100,
            "escode-session",
            "readSession",
            &Wire::Array(vec![Wire::Object(serde_json::json!({
                "workspacePath": workspace,
                "workspaceIdentity": workspace,
                "sessionId": task_id,
            }))]),
        )
        .await;
        let Wire::Object(snapshot) = read_response(&mut socket, 100).await else {
            panic!("readSession 应答应为对象")
        };
        if snapshot["session"]["status"].as_str() != Some("running") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "会话应在一轮失败后回到空闲: {snapshot}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let base = |id: i64, method: &str, extra: serde_json::Value| {
        let mut params = serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            params[key] = value;
        }
        (
            id,
            method.to_owned(),
            Wire::Array(vec![Wire::Object(params)]),
        )
    };

    // 标题生成可能仍占用 run（运行时 guard 在运行中拒绝模式切换）：先停掉它再切换。
    let (id, method, arg) = base(99, "stopGeneration", serde_json::json!({}));
    call(&mut socket, id, "escode-task", &method, &arg).await;
    assert_eq!(read_response(&mut socket, id).await, Wire::Undefined);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let mut switched = Wire::Undefined;
    for attempt in 0..30 {
        let (id, method, arg) = base(
            200 + attempt,
            "setMode",
            serde_json::json!({"mode": "plan"}),
        );
        call(&mut socket, id, "escode-task", &method, &arg).await;
        switched = read_response(&mut socket, id).await;
        if !matches!(&switched, Wire::Object(map) if map.get("name").is_some()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(switched, Wire::Undefined, "setMode 应最终成功");

    let (id, method, arg) = base(103, "getTaskConfigOptions", serde_json::json!({}));
    call(&mut socket, id, "escode-task", &method, &arg).await;
    let ids = options_array(read_response(&mut socket, id).await, "getTaskConfigOptions");
    assert!(!ids.is_empty(), "配置项不能为空: {ids:?}");

    let (id, method, arg) = base(
        104,
        "setModel",
        serde_json::json!({
            "modelSelection": {
                "providerId": "fixture", "modelId": "fixture-model",
                "options": {"reasoningLevel": "none"},
            },
        }),
    );
    call(&mut socket, id, "escode-task", &method, &arg).await;
    let ids = options_array(read_response(&mut socket, id).await, "setModel");
    assert!(ids.iter().any(|id| id == "model"), "{ids:?}");

    let (id, method, arg) = base(
        105,
        "setConfigOption",
        serde_json::json!({"configId": "thought_level", "value": "none"}),
    );
    call(&mut socket, id, "escode-task", &method, &arg).await;
    options_array(read_response(&mut socket, id).await, "setConfigOption");

    let (id, method, arg) = base(
        106,
        "setConfigOption",
        serde_json::json!({"configId": "mode", "value": "build"}),
    );
    call(&mut socket, id, "escode-task", &method, &arg).await;
    options_array(read_response(&mut socket, id).await, "setConfigOption mode");

    let (id, method, arg) = base(
        107,
        "setAutomationSessionConfig",
        serde_json::json!({
            "modelSelection": {"providerId": "fixture", "modelId": "fixture-model"},
            "thoughtLevel": "none",
        }),
    );
    call(&mut socket, id, "escode-task", &method, &arg).await;
    options_array(
        read_response(&mut socket, id).await,
        "setAutomationSessionConfig",
    );

    let (id, method, arg) = base(
        108,
        "setAssistantMessageFeedback",
        serde_json::json!({"turnIndex": 0, "feedback": "up"}),
    );
    call(&mut socket, id, "escode-task", &method, &arg).await;
    let snapshot = read_response(&mut socket, id)
        .await
        .into_object()
        .expect("feedback 应返回快照");
    assert_eq!(snapshot["meta"]["taskId"], serde_json::json!(task_id));

    for (index, method) in [
        (110, "switchAgent"),
        (111, "branchTaskFromPrompt"),
        (112, "deliverSessionMessage"),
        (113, "sendSessionMessageDeliveryResult"),
    ] {
        let (_, _, arg) = base(index, method, serde_json::json!({}));
        call(&mut socket, index, "escode-task", method, &arg).await;
        let error = read_response(&mut socket, index)
            .await
            .into_object()
            .expect("unsupported 错误载荷");
        assert_eq!(error["name"], serde_json::json!("Error"));
        assert_eq!(
            error["code"],
            serde_json::json!("ESCODE_AGENT_UNSUPPORTED_LEGACY_TASK_METHOD"),
            "{method}: {error}"
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("IESCodeTaskService.{method}")),
            "{error}"
        );
    }

    let (id, method, arg) = base(114, "closeTask", serde_json::json!({}));
    call(&mut socket, id, "escode-task", &method, &arg).await;
    assert_eq!(read_response(&mut socket, id).await, Wire::Undefined);
    surface.shutdown().await;
}
