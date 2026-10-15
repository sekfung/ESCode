//! `window-controller` 归档删除端到端（单个/批量：逐项目标结果、空批次短路、幂等跳过）。
//! 与 `rpc_window_controller.rs` 共用 `support::ControllerClient`。

mod support;

use serde_json::{Value, json};
use support::*;

fn object(wire: Wire) -> Value {
    wire.into_object().expect("object payload")
}

#[tokio::test(flavor = "multi_thread")]
async fn archived_deletions_report_per_task_results() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    let workspace_dir = dir.path().join("workspace");
    for path in [&home, &data_base_dir, &workspace_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    support::prepare_index(&data_base_dir);
    let workspace = workspace_dir.to_string_lossy().into_owned();
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&json!({"providerId": "fixture", "modelId": "fixture-model",
            "reasoningLevel": "none", "baseUrl": "http://127.0.0.1:9/v1"}))
        .unwrap(),
    )
    .unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut client = ControllerClient::new(open(&surface).await);
    let snapshot = object(
        client
            .call(
                "escode-session",
                "createSession",
                json!({"workspacePath": workspace, "workspaceIdentity": workspace,
                    "config": model_config.to_string_lossy(), "mode": "build",
                    "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
                    "persistence": "immediate"}),
            )
            .await
            .expect("createSession"),
    );
    let task_id = snapshot["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let address = json!({
        "workspacePath": workspace,
        "workspaceIdentity": workspace,
        "taskId": task_id,
    });

    // 先物化投影行（归档删除要求地址命中 source；行缺失时走 allowMissingTask 分支）。
    client
        .call(
            "window-controller",
            "listTaskList",
            json!({"kind": "active", "sortBy": "updated",
                "workspaceScopes": [{"workspacePath": workspace, "workspaceIdentity": workspace}]}),
        )
        .await
        .expect("listTaskList");

    // 空批次短路。
    let empty = object(
        client
            .call(
                "window-controller",
                "deleteArchivedTasks",
                json!({"address": address, "taskIds": []}),
            )
            .await
            .expect("empty batch"),
    );
    assert_eq!(empty["deletedTaskIds"].as_array().unwrap().len(), 0);

    // 未归档任务 → skipped（条件删除幂等跳过）。
    let skipped = object(
        client
            .call(
                "window-controller",
                "deleteArchivedTasks",
                json!({"address": address, "taskIds": [task_id]}),
            )
            .await
            .expect("batch delete"),
    );
    assert_eq!(
        skipped["skippedTaskIds"].as_array().unwrap(),
        &[Value::from(task_id.clone())]
    );
    assert_eq!(skipped["deletedTaskIds"].as_array().unwrap().len(), 0);

    // 归档后删除 → deleted。
    client
        .call(
            "window-controller",
            "mutateTask",
            json!({"address": address, "mutation": {"kind": "archive", "archived": true}}),
        )
        .await
        .expect("archive");
    let deleted = object(
        client
            .call(
                "window-controller",
                "deleteArchivedTask",
                json!({"address": address}),
            )
            .await
            .expect("delete archived"),
    );
    assert_eq!(deleted, Value::Bool(true));
}
