//! 会话域（`escode-session`）转发用例：读面透传运行时语义、写面显式缺口。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn session_list_forwards_to_the_workspace_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let surface = start(&home).await;
    let mut socket = connect(surface.addr, "/ws", None).await;

    // escode-session.listSessions：Host 把它转成 runtime 的 session/list（TS Host 同构）。
    call(
        &mut socket,
        21,
        "escode-session",
        "listSessions",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "workspaceIdentity": workspace.to_string_lossy(),
        }))]),
    )
    .await;
    let sessions = read_response(&mut socket, 21)
        .await
        .into_object()
        .expect("array");
    assert_eq!(sessions, serde_json::json!([]), "新 workspace 没有会话");

    // 实例已按 workspace 建好（/api/server-info 报告 workspaces）。
    let (_, info) = http_request(surface.addr, "GET", "/api/server-info", &[], None).await;
    let expected_workspace = workspace.to_string_lossy().into_owned();
    assert!(
        info["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str() == Some(expected_workspace.as_str())),
        "server-info 应报告已建实例: {info}"
    );
    assert!(
        info["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "escode-session"),
        "能力清单应含 escode-session: {info}"
    );

    // 读面转发：readSession / readSessionMessages（未激活会话由运行时按语义拒绝）。
    call(
        &mut socket,
        23,
        "escode-session",
        "readSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "workspaceIdentity": workspace.to_string_lossy(),
            "sessionId": "sess_missing",
        }))]),
    )
    .await;
    let error = read_response(&mut socket, 23)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "RuntimeError", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("Session unavailable"),
        "运行时语义透传: {error}"
    );
    call(
        &mut socket,
        24,
        "escode-session",
        "readSessionMessages",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "workspaceIdentity": workspace.to_string_lossy(),
            "sessionId": "sess_missing",
        }))]),
    )
    .await;
    let error = read_response(&mut socket, 24)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "RuntimeError", "{error}");
    assert!(
        error["message"].as_str().unwrap().contains("not active"),
        "运行时语义透传: {error}"
    );

    // 未迁移方法：显式缺口。
    call(
        &mut socket,
        22,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({}))]),
    )
    .await;
    let error = read_response(&mut socket, 22)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "NotImplemented");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("escode-session.createSession")
    );

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn session_writes_forward_legacy_methods() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let surface = start(&home).await;
    let mut socket = connect(surface.addr, "/ws", None).await;
    let path = workspace.to_string_lossy().into_owned();

    // closeSession → session/close：未加载的会话按运行时语义「条件关闭成立」（closed=true）。
    call(
        &mut socket,
        31,
        "escode-session",
        "closeSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
            "sessionId": "sess_missing",
        }))]),
    )
    .await;
    let closed = read_response(&mut socket, 31)
        .await
        .into_object()
        .expect("object");
    assert_eq!(closed, serde_json::json!(true), "closed 缺省为 true");

    // 条件关闭（expectedPersistence=deferred）对未加载会话为 false（运行时语义透传）。
    call(
        &mut socket,
        32,
        "escode-session",
        "closeSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
            "sessionId": "sess_missing",
            "expectedPersistence": "deferred",
        }))]),
    )
    .await;
    let closed = read_response(&mut socket, 32)
        .await
        .into_object()
        .expect("object");
    assert_eq!(closed, serde_json::json!(false));

    // setModel → session/setModel：运行时按会话语义拒绝（RuntimeError 透传）。
    call(
        &mut socket,
        33,
        "escode-session",
        "setModel",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
            "sessionId": "sess_missing",
            "model": {"providerId": "p", "modelId": "m"},
        }))]),
    )
    .await;
    let error = read_response(&mut socket, 33)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "RuntimeError", "{error}");

    // 未迁移的写方法：显式缺口。
    call(
        &mut socket,
        34,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
        }))]),
    )
    .await;
    let error = read_response(&mut socket, 34)
        .await
        .into_object()
        .expect("error");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("escode-session.createSession")
    );

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_presentation_reads_capabilities_first() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let surface = start(&home).await;
    let mut socket = connect(surface.addr, "/ws", None).await;
    let path = workspace.to_string_lossy().into_owned();

    call(
        &mut socket,
        41,
        "escode-session",
        "readWorkspacePresentation",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
        }))]),
    )
    .await;
    let presentation = read_response(&mut socket, 41)
        .await
        .into_object()
        .expect("object");
    // 运行时展示面字段（模式 + 斜杠命令目录）；执行能力位由运行时能力协商决定。
    assert_eq!(presentation["mode"], "build", "{presentation}");
    assert!(
        presentation["slashCommands"].is_array(),
        "展示面应带斜杠命令目录: {presentation}"
    );
    assert!(
        presentation["executionCapabilities"].is_object(),
        "运行时声明 workspaceExecutionCapabilities 时应带执行能力位: {presentation}"
    );

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_workspace_prewarms_and_reports_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let surface = start(&home).await;
    let mut socket = connect(surface.addr, "/ws", None).await;
    let path = workspace.to_string_lossy().into_owned();

    call(
        &mut socket,
        51,
        "escode-session",
        "initializeWorkspace",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
        }))]),
    )
    .await;
    let result = read_response(&mut socket, 51)
        .await
        .into_object()
        .expect("object");
    assert_eq!(result["available"], true, "{result}");
    assert_eq!(result["workspaceKey"], path);
    assert_eq!(result["protocolName"], "ESCode Protocol");
    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(result["transportKind"], "stdio");

    // 预热后实例已建（/api/server-info 可见）。
    let (_, info) = http_request(surface.addr, "GET", "/api/server-info", &[], None).await;
    assert!(
        info["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(path.as_str())),
        "预热应建立实例: {info}"
    );

    // 运行时起不来时仍是成功应答 + available:false（首屏不崩）：
    // data dir 指向一个文件（不是目录），存储初始化必然失败。
    let not_a_dir = dir.path().join("not-a-dir");
    std::fs::write(&not_a_dir, "x").unwrap();
    call(
        &mut socket,
        52,
        "escode-session",
        "initializeWorkspace",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": dir.path().join("broken").to_string_lossy(),
            "dataDir": not_a_dir.to_string_lossy(),
        }))]),
    )
    .await;
    let result = read_response(&mut socket, 52)
        .await
        .into_object()
        .expect("object");
    assert_eq!(result["available"], false, "{result}");
    assert_eq!(result["protocolName"], "ESCode Protocol");

    surface.shutdown().await;
}
