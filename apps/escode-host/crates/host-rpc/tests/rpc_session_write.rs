//! `escode-session` 写面与带 Host 职责的转发（真 WebSocket + 真 embedded 实例）。
//! 断言对齐 TS `escodeSessionService.ts` 的返回语义（评审 A11）。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn session_writes_forward_legacy_methods() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let surface = start(&home).await;
    let mut socket = open(&surface).await;
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
    // TS `closeSession` 回 `undefined`（`.then(() => undefined)`；旧实现回 `closed` 布尔，评审 A11）。
    assert_eq!(read_response(&mut socket, 31).await, Wire::Undefined);

    // expectedPersistence 随请求转发，结果仍是 undefined。
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
    assert_eq!(read_response(&mut socket, 32).await, Wire::Undefined);

    // 条件关闭的结果属 `closeDeferredDraftSession`：未加载会话 → false。
    call(
        &mut socket,
        36,
        "escode-session",
        "closeDeferredDraftSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
            "sessionId": "sess_missing",
        }))]),
    )
    .await;
    assert_eq!(
        read_response(&mut socket, 36).await,
        Wire::Object(serde_json::json!(false))
    );

    // `promoteDeferredDraftSession` 对非草稿是 no-op，回 undefined。
    call(
        &mut socket,
        37,
        "escode-session",
        "promoteDeferredDraftSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
            "sessionId": "sess_missing",
        }))]),
    )
    .await;
    assert_eq!(read_response(&mut socket, 37).await, Wire::Undefined);

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

    // createSession：转发到运行时（无模型配置时运行时语义错误透传，不是缺口回显）。
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
    assert_eq!(error["name"], "RuntimeError", "{error}");
    assert!(
        !error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("does not implement"),
        "createSession 必须转发到运行时: {error}"
    );

    // `resumeSession` 同：会话不存在 → 运行时 `Session unavailable` 透传。
    call(
        &mut socket,
        35,
        "escode-session",
        "resumeSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": path,
            "workspaceIdentity": path,
            "sessionId": "sess_missing",
        }))]),
    )
    .await;
    let error = read_response(&mut socket, 35)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "RuntimeError", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Session unavailable"),
        "resumeSession 必须把运行时语义错误透传: {error}"
    );

    surface.shutdown().await;
}
