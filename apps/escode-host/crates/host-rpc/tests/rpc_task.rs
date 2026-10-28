//! `escode-task` channel 端到端用例：`getTaskMeta` 直读 tasks-index（真实 WebSocket + 手写 RPC 编解码）。
//! 夹具按生成资产（`host-services::tasks_index::fixture::apply_generated_schema`）建库，与 TS Host 同物理 schema。
mod support;
use support::*;

/// 按 TS 迁移后的物理 DDL 建一个空库并写入一行任务。
fn seed(data_base_dir: &std::path::Path, workspace: &str, task_id: &str) -> std::path::PathBuf {
    let path = data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    escode_host_services::tasks_index::fixture::apply_generated_schema(
        &connection,
        escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS,
    )
    .unwrap();
    let meta = serde_json::json!({
        "taskId": task_id,
        "traceId": format!("escode-{task_id}"),
        "title": "评审 Rust Host",
        "workspacePath": workspace,
        "createdAt": 100,
        "updatedAt": 200,
        "mode": "build",
    });
    // provider 列写 glm、searchable_text 写正文：`listTaskList` 的 provider 过滤与摘要窗口要用到。
    connection
        .execute(
            "INSERT INTO tasks (
               workspace_key, workspace_path, task_id, title, mode, provider, created_at, updated_at,
               searchable_text, meta_json
             ) VALUES (?4, ?4, ?1, '列标题', 'build', 'glm', 100, 200, ?2, ?3)",
            rusqlite::params![
                task_id,
                "评审 Rust Host 的迁移进度",
                meta.to_string(),
                workspace
            ],
        )
        .unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn get_task_meta_reads_the_shared_index() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    seed(&data_base_dir, &workspace, "task_1");

    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        41,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "taskId": "task_1",
        }))]),
    )
    .await;
    let meta = read_response(&mut socket, 41)
        .await
        .into_object()
        .expect("object");
    assert_eq!(meta["taskId"], "task_1");
    assert_eq!(meta["title"], "评审 Rust Host", "meta_json 优先于列");
    assert_eq!(meta["workspacePath"], serde_json::json!(workspace));
    assert_eq!(meta["createdAt"], 100);
    assert_eq!(meta["titleOverridden"], false);

    // 不存在的任务：`undefined`（TS 返回 null，Wire 层同为「无值」）。
    call(
        &mut socket,
        42,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "taskId": "task_missing",
        }))]),
    )
    .await;
    assert!(matches!(
        read_response(&mut socket, 42).await,
        Wire::Undefined
    ));

    // listTaskList：kind 分区 + 搜索摘要 + 分页（`total` 为命中总数、`hasMore` 随之）。
    call(
        &mut socket,
        45,
        "escode-task",
        "listTaskList",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "kind": "timeline",
            "sortBy": "updated",
            "workspaceScopes": [{"workspacePath": workspace.clone(), "workspacePurpose": "project"}],
        }))]),
    )
    .await;
    let Wire::Object(list) = read_response(&mut socket, 45).await else {
        panic!("object")
    };
    assert_eq!(list["total"], 1, "{list}");
    assert_eq!(list["items"][0]["taskId"], "task_1");
    assert_eq!(list["items"][0]["workspacePurpose"], "project");

    call(
        &mut socket,
        46,
        "escode-task",
        "listTaskList",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "kind": "timeline",
            "sortBy": "updated",
            "workspaceScopes": [{"workspacePath": workspace.clone()}],
            "search": "评审",
            "limit": 1,
        }))]),
    )
    .await;
    let Wire::Object(searched) = read_response(&mut socket, 46).await else {
        panic!("object")
    };
    assert_eq!(searched["hasMore"], false, "{searched}");
    assert!(
        searched["items"][0]["searchSnippet"]
            .as_str()
            .unwrap_or_default()
            .contains("评审"),
        "{searched}"
    );

    // 分组结构：空索引下三个数组都在（结构形状稳定）。
    call(
        &mut socket,
        47,
        "escode-task",
        "listGroupedTaskViewStructure",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspaceScopes": [{"workspacePath": workspace.clone()}],
        }))]),
    )
    .await;
    let Wire::Object(structure) = read_response(&mut socket, 47).await else {
        panic!("object")
    };
    assert!(structure["groups"].as_array().is_some(), "{structure}");
    assert!(structure["members"].as_array().is_some(), "{structure}");
    assert!(
        structure["topLevelOrders"].as_array().is_some(),
        "{structure}"
    );

    // getTaskModelSelection：`{taskId}` 入参靠**任务目标注册表**解析 workspace。
    // 1) 未注册的任务：TS 同款结构化错误（code + 文案）。
    call(
        &mut socket,
        51,
        "escode-task",
        "getTaskModelSelection",
        &Wire::Array(vec![Wire::Object(
            serde_json::json!({"taskId": "task_never_seen"}),
        )]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 51).await else {
        panic!("error object")
    };
    assert_eq!(error["name"], "ESCODE_SESSION_TARGET_NOT_FOUND", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("target is not loaded: task_never_seen"),
        "{error}"
    );

    // 2) 列表读登记目标后，调用被路由到该 workspace 的运行时会话读；
    //    这里没有活跃会话，运行时按语义拒绝（证明注册表 → 运行时路由打通）。
    call(
        &mut socket,
        52,
        "escode-task",
        "listTasks",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
        }))]),
    )
    .await;
    let listed = read_response(&mut socket, 52).await;
    assert!(matches!(listed, Wire::Array(_)), "列表读成功");
    call(
        &mut socket,
        53,
        "escode-task",
        "getTaskModelSelection",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"taskId": "task_1"}))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 53).await else {
        panic!("error object")
    };
    assert_eq!(error["name"], "ESCodeProtocolClientError", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Session"),
        "运行时会话读的语义错误应透传: {error}"
    );

    // 未迁移方法：显式缺口（列出已实现的方法）。escode-task 已 50/50 收口，
    // 这里用仍在迁移中的 escode-agent 方法验证同一缺口语义。
    call(
        &mut socket,
        43,
        "escode-agent",
        "readSessionEvents",
        &Wire::Array(vec![Wire::Object(serde_json::json!({}))]),
    )
    .await;
    let error = read_response(&mut socket, 43).await;
    let Wire::Object(error) = error else {
        panic!("error object: {error:?}")
    };
    let message = error["message"].as_str().unwrap_or_default().to_owned();
    assert!(
        message.contains("does not implement `escode-agent.readSessionEvents` yet"),
        "{message}"
    );
    assert!(message.contains("helloConversationV4"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_task_meta_reports_a_structured_error_when_the_index_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // dataBaseDir 存在但 TS Host 尚未建库。
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        44,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": "/w",
            "taskId": "task_1",
        }))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 44).await else {
        panic!("error object")
    };
    // RPC 错误帧按 JS Error 形状下发：结构化 code 落在 `name`。
    assert_eq!(error["name"], "TasksIndexUnavailable", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tasks-index is missing"),
        "{error}"
    );
}
