//! `task-groups` channel 端到端用例：登记 / 移动 / 过期来源拒绝 / 解组（docs/specs/rust-project-storage.md §3.3）。
mod support;
use support::*;

use serde_json::{Value, json};

async fn start_host() -> (tempfile::TempDir, Socket, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    prepare_index(&data_base_dir);
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    (dir, open(&surface).await, data_base_dir)
}

fn message(wire: Wire) -> String {
    let Wire::Object(error) = wire else {
        panic!("expected error object, got {wire:?}")
    };
    error["message"].as_str().unwrap_or_default().to_owned()
}

async fn call_groups(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    let arg = if params.is_null() {
        Wire::Array(Vec::new())
    } else {
        Wire::Array(vec![Wire::Object(params)])
    };
    call(socket, id, "task-groups", method, &arg).await;
    read_response(socket, id).await
}

#[tokio::test]
async fn task_groups_admit_move_and_ungroup_the_indexed_organization() {
    let (dir, mut socket, data_base_dir) = start_host().await;
    let workspace = dir.path().join("group-workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let index_path = data_base_dir.join(".escode/v2/tasks-index.sqlite");
    let connection = rusqlite::Connection::open(&index_path).unwrap();
    connection
        .execute(
            "INSERT INTO tasks(workspace_key,workspace_path,task_id,title,mode,created_at,updated_at,
               host_id,root_path,cli_created_at,cli_updated_at)
             VALUES(?1,?1,'task_group','分组','build',100,200,'local',?1,100,200)",
            rusqlite::params![workspace.to_string_lossy()],
        )
        .unwrap();
    drop(connection);

    let task = json!({
        "hostId": "local",
        "workspacePath": workspace.to_string_lossy(),
        "taskId": "task_group",
    });
    call_groups(&mut socket, 1, "admit", json!({"tasks": [task]})).await;
    let Wire::Object(group) = call_groups(&mut socket, 2, "save", json!({"title": "组"})).await
    else {
        panic!("save object")
    };
    let group_id = group["id"].as_str().unwrap().to_owned();
    call_groups(
        &mut socket,
        3,
        "move",
        json!({
            "node": {"type": "task", "task": task},
            "sourceGroupId": null,
            "groupId": group_id,
        }),
    )
    .await;
    let Wire::Object(structure) = call_groups(&mut socket, 4, "read", json!(null)).await else {
        panic!("read object")
    };
    assert_eq!(structure["groups"].as_array().unwrap().len(), 1);
    assert_eq!(structure["members"].as_array().unwrap().len(), 1);

    // 来源组已过期：另一客户端已整理，旧 UI 不能覆盖。
    let error = message(
        call_groups(
            &mut socket,
            5,
            "move",
            json!({
                "node": {"type": "task", "task": task},
                "sourceGroupId": null,
                "groupId": group_id,
            }),
        )
        .await,
    );
    assert!(error.contains("task-group.stale-source"), "{error}");

    call_groups(&mut socket, 6, "ungroup", json!(group_id)).await;
    let Wire::Object(structure) = call_groups(&mut socket, 7, "read", json!(null)).await else {
        panic!("read object")
    };
    assert!(structure["groups"].as_array().unwrap().is_empty());
    assert!(structure["members"].as_array().unwrap().is_empty());
    assert!(
        structure["order"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["type"] == "task" && node["key"].to_string().contains("task_group"))
    );
}
