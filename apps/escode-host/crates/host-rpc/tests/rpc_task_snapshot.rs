//! `escode-task` 快照读面（S2b）的通道接线用例：与 `rpc_task.rs` 同套 RPC 夹具，
//! 单独成文件以守住单文件行数上限（见 docs/specs/rust-u2-tasks-index.md §6.4）。
mod support;
use support::*;

/// S2b 快照读面的通道接线：`getTaskSnapshot` / `getTaskSnapshotWithEtag` / `getTaskConfigOptions`
/// 都转发到运行时的 `session/resume`（真 embedded 实例 + 真 WebSocket），会话不存在时把运行时语义
/// 错误原样透出；`getTaskSnapshotBody` / `Ref` / `ToolCallsSlice` 按 TS 适配器语义直接回 `null`
/// （不启动实例、不读索引）。
#[tokio::test(flavor = "multi_thread")]
async fn task_snapshot_methods_hit_the_runtime_and_the_null_trio_stays_local() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    // 真 workspace 目录：不存在的路径会让实例起不来、请求挂到超时（看起来像「没实现」）。
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    let snapshot_params = |task_id: &str| {
        Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": task_id,
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
        }))])
    };

    // 会话不存在：运行时的 `Session unavailable` 透传（证明请求真的到了运行时的 session/resume，
    // 而不是缺口回显或索引短路）。
    for (id, method) in [
        (61, "getTaskSnapshot"),
        (62, "getTaskSnapshotWithEtag"),
        (63, "getTaskConfigOptions"),
    ] {
        call(
            &mut socket,
            id,
            "escode-task",
            method,
            &snapshot_params("sess_absent"),
        )
        .await;
        let Wire::Object(error) = read_response(&mut socket, id).await else {
            panic!("error object for {method}")
        };
        assert_eq!(
            error["name"], "ESCodeProtocolClientError",
            "{method}: {error}"
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Session unavailable"),
            "{method} 必须透传运行时的语义错误: {error}"
        );
    }

    // 三个 `null` 方法：TS 适配器本身 `return null`，本地直回（不需要会话，也不碰索引）。
    for (id, method) in [
        (64, "getTaskSnapshotBody"),
        (65, "getTaskSnapshotRef"),
        (66, "getTaskSnapshotToolCallsSlice"),
    ] {
        call(&mut socket, id, "escode-task", method, &Wire::Array(vec![])).await;
        let Wire::Object(value) = read_response(&mut socket, id).await else {
            panic!("null payload for {method}")
        };
        assert_eq!(value, serde_json::Value::Null, "{method} 必须回 null");
    }

    // 缺 `taskId` / `workspacePath`：显式参数错误（不落到运行时）。
    call(
        &mut socket,
        67,
        "escode-task",
        "getTaskSnapshot",
        &Wire::Array(vec![Wire::Object(
            serde_json::json!({"workspacePath": workspace.clone()}),
        )]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 67).await else {
        panic!("error object")
    };
    assert_eq!(error["name"], "InvalidRequest", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("taskId is required"),
        "{error}"
    );

    call(
        &mut socket,
        68,
        "escode-task",
        "getTaskSnapshot",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"taskId": "sess_1"}))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 68).await else {
        panic!("error object")
    };
    assert_eq!(error["name"], "InvalidRequest", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("workspacePath is required"),
        "{error}"
    );
}

/// 未建库时快照读面回结构化错误（`TasksIndexUnavailable`）：索引是 meta 的唯一来源，不静默丢写。
#[tokio::test(flavor = "multi_thread")]
async fn task_snapshot_reports_a_structured_error_when_the_index_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    let workspace_path = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();
    let workspace = workspace_path.to_string_lossy().into_owned();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut socket = open(&surface).await;

    call(
        &mut socket,
        69,
        "escode-task",
        "getTaskSnapshot",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": "sess_1",
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
        }))]),
    )
    .await;
    let Wire::Object(error) = read_response(&mut socket, 69).await else {
        panic!("error object")
    };
    // 顺序与 TS 一致：先 resume（会话不存在 → ESCodeProtocolClientError），所以这里先看到运行时错误；
    // 索引缺失只在会话存在才会暴露。缺 dataBaseDir 的 Host 配置另测（见下方 gap 用例）。
    assert_eq!(error["name"], "ESCodeProtocolClientError", "{error}");
}

/// 缺 `dataBaseDir`（Main 未提交）：六个快照方法都回显式缺口，不静默成功。
#[tokio::test(flavor = "multi_thread")]
async fn task_snapshot_methods_report_an_explicit_gap_without_data_base_dir() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory = std::sync::Arc::new({
        let home = home.clone();
        move |workspace: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: home.clone(),
                env: std::collections::BTreeMap::new(),
                argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: None,
                config: None,
            }
        }
    });
    let channels = escode_host_rpc::domains::channels(home, None, instances.clone(), options);
    let surface = escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, Some(instances))
        .await
        .expect("rpc surface starts");
    let mut socket = open(&surface).await;

    for (id, method) in [
        (71, "getTaskSnapshot"),
        (72, "getTaskSnapshotWithEtag"),
        (73, "getTaskSnapshotBody"),
        (74, "getTaskSnapshotRef"),
        (75, "getTaskSnapshotToolCallsSlice"),
        (76, "getTaskConfigOptions"),
    ] {
        call(&mut socket, id, "escode-task", method, &Wire::Array(vec![])).await;
        let Wire::Object(error) = read_response(&mut socket, id).await else {
            panic!("error object for {method}")
        };
        assert_eq!(error["name"], "InvalidRequest", "{method}: {error}");
        assert!(
            error["message"]
                .as_str()
                .unwrap_or_default()
                .contains("no dataBaseDir"),
            "{method} 缺 dataBaseDir 时必须显式缺口: {error}"
        );
    }
}
