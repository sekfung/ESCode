use super::*;
use std::path::Path;

/// 夹具：按生成资产建库并写入若干行（provider/pinned/archived/updated_at 组合各异）。
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    for statement in escode_host_services::tasks_index::schema::MIGRATED_DDL {
        connection.execute_batch(statement).unwrap();
    }
    for (id, checksum) in escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS {
        connection
            .execute(
                "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
                rusqlite::params![id, checksum],
            )
            .unwrap();
    }
    // (task_id, provider, pinned, archived, deleted, updated_at)
    let rows = [
        ("task_a", Some("glm"), 0, 0, 0, 100),
        ("task_b", Some("glm"), 1, 0, 0, 200),
        ("task_c", Some("glm"), 0, 1, 0, 300),
        ("task_d", Some("glm"), 0, 0, 0, 400),
        ("task_e", Some("glm"), 0, 0, 1, 500),
        // 历史 provider（claude 导入）：列表按 glm 过滤时不可见（TS 同）。
        ("task_legacy", Some("claude"), 0, 0, 0, 600),
        // provider 为空：同样被 glm 过滤排除。
        ("task_no_provider", None, 0, 0, 0, 700),
    ];
    for (task_id, provider, pinned, archived, deleted, updated_at) in rows {
        let meta = serde_json::json!({
            "taskId": task_id,
            "traceId": format!("escode-{task_id}"),
            "title": format!("标题 {task_id}"),
            "workspacePath": "/w",
            "createdAt": 1,
            "updatedAt": updated_at,
            "mode": "build",
        });
        connection
            .execute(
                "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, mode, provider,
                   created_at, updated_at, pinned, archived, deleted, meta_json)
                 VALUES ('/w', '/w', ?1, ?2, 'build', ?3, 1, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    task_id,
                    format!("标题 {task_id}"),
                    provider,
                    updated_at,
                    pinned,
                    archived,
                    deleted,
                    meta.to_string()
                ],
            )
            .unwrap();
    }
    (dir, path)
}

fn channel_for(path: &Path) -> Channel {
    // `<dataBaseDir>/.escode/v2/tasks-index.sqlite` → dataBaseDir 是上三级。
    let data_base_dir = path.parent().unwrap().parent().unwrap().parent().unwrap();
    task_channel(
        Some(data_base_dir.to_path_buf()),
        None,
        Arc::new(Mutex::new(LazyIndex::default())),
        Arc::new(TaskTargets::new()),
    )
}

async fn call(channel: &Channel, method: &str, params: Value) -> Result<Wire, (String, String)> {
    let handler = channel.handler(method).expect("method registered");
    handler(Wire::Array(vec![Wire::Object(params)]))
        .await
        .map_err(Into::into)
}

fn names(wire: Wire) -> Vec<String> {
    let Wire::Array(entries) = wire else {
        panic!("array")
    };
    entries
        .iter()
        .map(|entry| {
            let Wire::Object(meta) = entry else {
                panic!("object")
            };
            meta["taskId"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[tokio::test]
async fn list_partitions_follow_the_ts_filters_and_order() {
    let (_dir, path) = fixture();
    let channel = channel_for(&path);
    let workspace = serde_json::json!({"workspacePath": "/w"});

    // listTasks：glm + 未 pinned + 未归档，按 updated_at DESC。
    assert_eq!(
        names(
            call(&channel, "listTasks", workspace.clone())
                .await
                .unwrap()
        ),
        vec!["task_d", "task_a"]
    );
    // listPinnedTasks：pinned 行。
    assert_eq!(
        names(
            call(&channel, "listPinnedTasks", workspace.clone())
                .await
                .unwrap()
        ),
        vec!["task_b"]
    );
    // listArchivedTasks：归档行（TS 不过滤 pinned）。
    assert_eq!(
        names(
            call(&channel, "listArchivedTasks", workspace.clone())
                .await
                .unwrap()
        ),
        vec!["task_c"]
    );
    // listPinnedTaskIds：无 workspace 入参（全局 pinned 真相源）。
    let Wire::Array(ids) = call(&channel, "listPinnedTaskIds", Value::Null)
        .await
        .unwrap()
    else {
        panic!("array")
    };
    assert_eq!(
        ids.iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["task_b"]
    );
    // listDeletedTaskIds：deleted=1 且 glm，按 id 升序。
    let Wire::Array(deleted) = call(&channel, "listDeletedTaskIds", workspace.clone())
        .await
        .unwrap()
    else {
        panic!("array")
    };
    assert_eq!(
        deleted
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["task_e"]
    );
}

#[tokio::test]
async fn missing_data_base_dir_reports_an_explicit_gap_for_every_read() {
    let channel = task_channel(
        None,
        None,
        Arc::new(Mutex::new(LazyIndex::default())),
        Arc::new(TaskTargets::new()),
    );
    for method in [
        "getTaskMeta",
        "listTasks",
        "listPinnedTaskIds",
        "listPinnedTasks",
        "listDeletedTaskIds",
        "listArchivedTasks",
        "listTaskList",
        "listGroupedTaskViewStructure",
        "getTaskModelSelection",
    ] {
        let error = call(&channel, method, serde_json::json!({}))
            .await
            .expect_err("explicit gap");
        assert_eq!(error.0, "InvalidRequest", "{method}");
        assert!(error.1.contains("Main must pass dataBaseDir"), "{method}");
    }
}
