//! `usage-stats.getAppUsageSnapshot` 端到端（真 embedded 实例 + 真 WebSocket）。
//!
//! 证明两件不能用单测覆盖的事（子 spec `rust-u2-misc-channels.md` §1.4）：
//! 1. **existing-only**：没有任何工作区用过运行时时直接报 `no_active_workspace`——这条查询**不为**自己
//!    拉起运行时（若实现误用 start-if-needed，第 71 号请求会等到实例起来而拿不到该错误）；
//! 2. 拉起运行时后同一调用**真的走 `v4/usage/stats`**，返回 `AppUsageSnapshot` 形状（不是缺口回显、
//!    也不是 Host 本地造的假快照）。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn usage_snapshot_is_existing_only_and_then_reads_the_runtime() {
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

    // UI 的调用形状：只有 range / timeZone（`useUsageStats.ts:44`），不带 workspace。
    let snapshot_args = || {
        Wire::Array(vec![Wire::Object(serde_json::json!({
            "range": "7d",
            "timeZone": "UTC",
        }))])
    };

    // 1) 还没有活实例：不拉起运行时，按 TS 同形报 `no_active_workspace`。
    call(
        &mut socket,
        71,
        "usage-stats",
        "getAppUsageSnapshot",
        &snapshot_args(),
    )
    .await;
    let error = read_response(&mut socket, 71)
        .await
        .into_object()
        .expect("error payload");
    assert_eq!(error["name"], serde_json::json!("Error"), "{error}");
    assert_eq!(
        error["message"],
        serde_json::json!("no_active_workspace"),
        "{error}"
    );

    // 2) 用真会话拉起该 workspace 的运行时。
    call(
        &mut socket,
        72,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.clone(),
            "workspaceIdentity": workspace.clone(),
            "config": model_config.to_string_lossy(),
            "mode": "build",
            "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
            "persistence": "immediate",
        }))]),
    )
    .await;
    let Wire::Object(created) = read_response(&mut socket, 72).await else {
        panic!("createSession 应答不是快照对象")
    };
    assert!(
        created["session"]["sessionId"].as_str().is_some(),
        "运行时未建出会话：{created}"
    );

    // 3) 同一调用现在返回真实统计快照。
    call(
        &mut socket,
        73,
        "usage-stats",
        "getAppUsageSnapshot",
        &snapshot_args(),
    )
    .await;
    let Wire::Object(snapshot) = read_response(&mut socket, 73).await else {
        panic!("usage 快照不是对象")
    };
    assert_eq!(snapshot["range"], serde_json::json!("7d"), "{snapshot}");
    assert_eq!(snapshot["timeZone"], serde_json::json!("UTC"), "{snapshot}");
    assert_eq!(
        snapshot["source"],
        serde_json::json!("agent-db"),
        "快照必须来自 agent 库统计：{snapshot}"
    );
    for key in ["summary", "heatmap", "dailyModelUsage", "models", "tools"] {
        assert!(snapshot.get(key).is_some(), "快照缺 `{key}`：{snapshot}");
    }
    // 空库（无回合/工具记录）统计为零，且 timeZone 原样回传说明参数真的到了运行时。
    assert_eq!(
        snapshot["summary"]["totalTokens"],
        serde_json::json!(0),
        "{snapshot}"
    );

    surface.shutdown().await;
}
