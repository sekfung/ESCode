//! `escode-session.createSession` / `resumeSession` 的端到端用例（真 embedded 实例 + 真 WebSocket）：
//! 建会话 → 索引行 → 快照读面 → deferred 草稿不进索引。与 `rpc_task_snapshot.rs` 同套夹具，
//! 单独成文件以守住单文件行数上限。
mod support;
use support::*;

/// 真运行时建会话 → 索引行 → 快照读面（S2b 的端到端补口）：证明写入口与读面在同一条链路上，
/// 且 `persistence: "deferred"` 的草稿**不进索引**（子 spec §6 S3 的负向断言）。
#[tokio::test(flavor = "multi_thread")]
async fn create_session_writes_the_index_and_snapshot_reads_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    support::prepare_index(&data_base_dir);
    // 模型配置夹具（与 CLI `--config` 同格式）：baseUrl 指向必然拒绝的端口，本用例不发模型请求。
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

    let create_params = |persistence: &str| {
        Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
            "config": model_config.to_string_lossy(),
            "mode": "build",
            "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
            "persistence": persistence,
            // 白名单外的键必须被整形丢掉（否则运行时会按未知字段拒绝）。
            "deliveryKind": "desktop-continuous",
            "sessionTraceId": "trace-fixture",
        }))])
    };

    call(
        &mut socket,
        81,
        "escode-session",
        "createSession",
        &create_params("immediate"),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 81).await else {
        panic!("snapshot object")
    };
    let session_id = match snapshot["session"]["sessionId"].as_str() {
        Some(id) => id.to_owned(),
        None => panic!("createSession 应答不是快照：{snapshot}"),
    };
    assert!(
        session_id.starts_with("sess_"),
        "运行时会话 id 形如 sess_<id>：{session_id}"
    );
    assert_eq!(snapshot["session"]["mode"], serde_json::json!("build"));

    // 写入口已经把初始快照写进索引（否则侧边栏要等首条 topic 帧）。
    call(
        &mut socket,
        82,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
            "taskId": session_id.clone(),
        }))]),
    )
    .await;
    let Wire::Object(meta) = read_response(&mut socket, 82).await else {
        panic!("meta object")
    };
    assert_eq!(meta["taskId"], serde_json::json!(session_id));
    assert_eq!(meta["provider"], serde_json::json!("glm"));
    assert_eq!(meta["mode"], serde_json::json!("build"));
    // 新会话没有用户输入：标题回退到 session.title 或 "New session"（TS 同回退链）。
    assert!(
        meta["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()),
        "{meta}"
    );

    // 快照读面：投影 + 索引行同源（`meta` 就是索引行）。
    call(
        &mut socket,
        83,
        "escode-task",
        "getTaskSnapshot",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": session_id.clone(),
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
            "clientMode": "desktop-continuous",
        }))]),
    )
    .await;
    let Wire::Object(projected) = read_response(&mut socket, 83).await else {
        panic!("snapshot projection")
    };
    assert_eq!(projected["meta"]["taskId"], serde_json::json!(session_id));
    assert_eq!(projected["meta"]["mode"], serde_json::json!("build"));
    assert!(projected["messages"].as_array().is_some_and(Vec::is_empty));
    assert!(
        projected["fileChanges"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    assert_eq!(
        projected["configOptions"][0]["id"],
        serde_json::json!("model")
    );
    assert_eq!(
        projected["runtime"]["pendingCommands"],
        serde_json::json!([])
    );
    // 未截窗：不写 `history`（只有 messageLimit 生效时才出现）。
    assert!(projected.get("history").is_none(), "{projected}");

    // ETag：形态是 64 位 hex；`ifNoneMatch` 未命中时回 `{snapshot, etag}`。
    call(
        &mut socket,
        84,
        "escode-task",
        "getTaskSnapshotWithEtag",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": session_id.clone(),
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
        }))]),
    )
    .await;
    let Wire::Object(etagged) = read_response(&mut socket, 84).await else {
        panic!("etag payload")
    };
    let etag = etagged["etag"].as_str().expect("etag").to_owned();
    assert_eq!(etag.len(), 64);
    assert!(etag.chars().all(|character| character.is_ascii_hexdigit()));
    assert!(etagged["snapshot"]["meta"]["taskId"].is_string());

    // deferred 草稿：同样建会话，但**不进索引**（子 spec §6 S3 负向断言）。
    call(
        &mut socket,
        85,
        "escode-session",
        "createSession",
        &create_params("deferred"),
    )
    .await;
    let Wire::Object(draft) = read_response(&mut socket, 85).await else {
        panic!("draft snapshot")
    };
    let draft_id = draft["session"]["sessionId"]
        .as_str()
        .expect("draft sessionId")
        .to_owned();
    assert_ne!(draft_id, session_id);
    call(
        &mut socket,
        86,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
            "taskId": draft_id.clone(),
        }))]),
    )
    .await;
    assert!(
        matches!(read_response(&mut socket, 86).await, Wire::Undefined),
        "deferred 草稿不能出现在任务索引里"
    );

    // 列表读只看到非草稿的那一条。
    call(
        &mut socket,
        87,
        "escode-task",
        "listTasks",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
        }))]),
    )
    .await;
    let Wire::Array(tasks) = read_response(&mut socket, 87).await else {
        panic!("task list")
    };
    let ids = tasks
        .iter()
        .filter_map(|task| task.clone().into_object())
        .filter_map(|task| task["taskId"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![session_id.clone()], "草稿不进侧边栏列表");
}
