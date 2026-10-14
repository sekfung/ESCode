//! `file` 域端到端用例（真实 WebSocket + 手写 RPC 编解码）。
//! 覆盖需要 Host 路径配置的方法：会话工作区取 Main 提交的 dataBaseDir（不是 HOME）。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn conversation_workspace_uses_the_main_provided_data_base_dir() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // dataBaseDir 故意与 home 不同：Host 必须用 Main 提交的值，而不是 HOME 派生。
    let data_base_dir = dir.path().join("data-base");
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = connect(surface.addr, "/ws", None).await;

    call(
        &mut socket,
        31,
        "file",
        "ensureConversationWorkspace",
        &Wire::Array(vec![Wire::Undefined]),
    )
    .await;
    let created = read_response(&mut socket, 31)
        .await
        .into_object()
        .expect("object");
    let expected = data_base_dir
        .join(".escode")
        .join("workspace")
        .join("default");
    assert_eq!(created["path"], serde_json::json!(expected.to_string_lossy()));
    assert_eq!(created["created"], serde_json::json!(true));
    assert_eq!(
        created["workspacePurpose"],
        serde_json::json!("conversation")
    );
    assert!(expected.is_dir(), "递归建出会话工作区");

    // 幂等：再次调用 created=false（Node recursive mkdir 返回 undefined）。
    call(
        &mut socket,
        32,
        "file",
        "ensureConversationWorkspace",
        &Wire::Array(vec![Wire::Undefined]),
    )
    .await;
    let second = read_response(&mut socket, 32)
        .await
        .into_object()
        .expect("object");
    assert_eq!(second["created"], serde_json::json!(false));

    // 默认/命名工作区仍按 home（homedir 语义）派生，与 dataBaseDir 无关。
    call(
        &mut socket,
        33,
        "file",
        "createDefaultWorkspace",
        &Wire::Array(vec![Wire::Undefined]),
    )
    .await;
    let default_workspace = read_response(&mut socket, 33)
        .await
        .into_object()
        .expect("object");
    assert_eq!(
        default_workspace["path"],
        serde_json::json!(home.join("ESCodeProject").to_string_lossy())
    );
}
