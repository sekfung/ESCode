//! `subagents` 端到端（真 WebSocket，`rust-u2-subagents.md` §4）。
//!
//! 覆盖已交付的 4/8：目录解析 + 状态写入面；未迁移的 4 个必须回**显式缺口**并列出已实现集合。
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

    // ── 未迁移的 4 个：显式缺口 + 已实现集合 ────────────────────────
    for (id, method) in [
        (9, "list"),
        (10, "createAgent"),
        (11, "updateAgent"),
        (12, "deleteAgent"),
    ] {
        let error = call_json(&mut socket, id, method, serde_json::json!({}))
            .await
            .into_object()
            .expect("缺口载荷");
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&format!("does not implement `subagents.{method}`")),
            "{method} 应是显式缺口: {error}"
        );
        assert!(
            message.contains("setBuiltInModelOverride"),
            "{method} 的缺口应列出已实现集合: {error}"
        );
    }

    surface.shutdown().await;
}

fn read_state(path: &std::path::Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".to_owned());
    serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
}
