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
    let mut socket = open(&surface).await;

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
    let (_, info) = http_request(
        surface.addr,
        "GET",
        "/api/server-info",
        &secret_header(&surface),
        None,
    )
    .await;
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
    assert_eq!(error["name"], "ESCodeProtocolClientError", "{error}");
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
    assert_eq!(error["name"], "ESCodeProtocolClientError", "{error}");
    assert!(
        error["message"].as_str().unwrap().contains("not active"),
        "运行时语义透传: {error}"
    );

    // 写入口已迁移：`createSession` 转发到运行时的 `session/create`（不再是 NotImplemented）。
    // 该 workspace 没有模型配置夹具，运行时按会话语义拒绝——错误来自运行时而不是缺口回显，
    // 证明参数整形后的请求真的到了 runtime。
    call(
        &mut socket,
        22,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "workspaceIdentity": workspace.to_string_lossy(),
        }))]),
    )
    .await;
    let error = read_response(&mut socket, 22)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "ESCodeProtocolClientError", "{error}");
    assert!(
        !error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("does not implement"),
        "写入口必须转发到运行时: {error}"
    );
    // 缺 workspacePath：参数校验（不落到运行时）。
    call(
        &mut socket,
        25,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"mode": "build"}))]),
    )
    .await;
    let error = read_response(&mut socket, 25)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "InvalidRequest", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("workspacePath is required"),
        "{error}"
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
    let mut socket = open(&surface).await;
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
    let mut socket = open(&surface).await;
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
    let (_, info) = http_request(
        surface.addr,
        "GET",
        "/api/server-info",
        &secret_header(&surface),
        None,
    )
    .await;
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

#[tokio::test(flavor = "multi_thread")]
async fn system_info_reports_home_and_node_platform() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let surface = start(&home).await;
    let mut socket = open(&surface).await;

    call(&mut socket, 61, "system", "info", &Wire::Array(Vec::new())).await;
    let info = read_response(&mut socket, 61)
        .await
        .into_object()
        .expect("object");
    let expected_home = home.to_string_lossy().into_owned();
    assert_eq!(
        info["homedir"].as_str(),
        Some(expected_home.as_str()),
        "{info}"
    );
    let expected_platform = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    };
    assert_eq!(info["platform"], expected_platform, "{info}");

    // 未迁移方法：显式缺口。
    call(
        &mut socket,
        62,
        "system",
        "listIntegratedTerminalShells",
        &Wire::Array(Vec::new()),
    )
    .await;
    let error = read_response(&mut socket, 62)
        .await
        .into_object()
        .expect("error");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("system.listIntegratedTerminalShells")
    );

    surface.shutdown().await;
}
