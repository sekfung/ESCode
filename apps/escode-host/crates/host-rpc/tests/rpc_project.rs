//! `project` channel 端到端用例：`readDirectory` / `prepareSession` / `saveProject` 目录校验 /
//! `readProjects` 主目录状态 / `moveTask` 缺 root 确认（docs/specs/rust-project-storage.md §4）。
mod support;
use support::*;

use serde_json::{Value, json};

async fn start_project_host() -> (tempfile::TempDir, RpcSurface, Socket, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data_base_dir = dir.path().join("data-base");
    std::fs::create_dir_all(&data_base_dir).unwrap();
    prepare_index(&data_base_dir);
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let socket = open(&surface).await;
    (dir, surface, socket, data_base_dir)
}

fn message(wire: Wire) -> String {
    let Wire::Object(error) = wire else {
        panic!("expected error object, got {wire:?}")
    };
    error["message"].as_str().unwrap_or_default().to_owned()
}

fn to_value(wire: Wire) -> Value {
    match wire {
        Wire::Undefined => Value::Null,
        Wire::Str(value) => json!(value),
        Wire::Int(value) => json!(value),
        Wire::Object(value) => value,
        Wire::Array(values) => Value::Array(values.into_iter().map(to_value).collect()),
        Wire::Bytes { data, .. } => json!(data),
    }
}

async fn call_project(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    call_channel(socket, id, "project", method, params).await
}

async fn call_channel(
    socket: &mut Socket,
    id: i64,
    channel: &str,
    method: &str,
    params: Value,
) -> Wire {
    let arg = if params.is_null() {
        Wire::Array(Vec::new())
    } else {
        Wire::Array(vec![Wire::Object(params)])
    };
    call(socket, id, channel, method, &arg).await;
    read_response(socket, id).await
}

#[tokio::test]
async fn prepare_session_reuses_draft_and_initializes_empty_project_roots() {
    let (dir, _surface, mut socket, data_base_dir) = start_project_host().await;
    let draft = "cc29167e-5a72-4c24-9e31-adcc584c73b4";
    let workspace_dir = data_base_dir.join(".escode/workspace/default");

    let Wire::Object(first) =
        call_project(&mut socket, 1, "prepareSession", json!({"draftId": draft})).await
    else {
        panic!("prepareSession object")
    };
    let Wire::Object(second) =
        call_project(&mut socket, 2, "prepareSession", json!({"draftId": draft})).await
    else {
        panic!("prepareSession object")
    };
    assert_eq!(first["workspacePath"], second["workspacePath"]);
    assert_eq!(
        first["workspacePath"],
        json!(
            workspace_dir
                .join(format!("draft-{draft}"))
                .to_string_lossy()
        )
    );
    assert_eq!(first["projectId"], Value::Null);

    // 无 draftId 的调用独立分配，不复用最近目录。
    let Wire::Object(allocated) = call_project(&mut socket, 3, "prepareSession", json!({})).await
    else {
        panic!("prepareSession object")
    };
    assert_ne!(allocated["workspacePath"], first["workspacePath"]);
    assert!(std::path::Path::new(allocated["workspacePath"].as_str().unwrap()).is_dir());

    // 非法 draftId 拒绝且不建目录。
    let entries_before = std::fs::read_dir(&workspace_dir).unwrap().count();
    let error = message(
        call_project(
            &mut socket,
            4,
            "prepareSession",
            json!({"draftId": "../../escape"}),
        )
        .await,
    );
    assert!(error.contains("project.invalid-draft-target"), "{error}");
    assert_eq!(
        std::fs::read_dir(&workspace_dir).unwrap().count(),
        entries_before
    );

    // reuseDraft 目录被移走后只报缺失，不用 mkdir 复活。
    let draft_path = workspace_dir.join(format!("draft-{draft}"));
    std::fs::remove_dir_all(&draft_path).unwrap();
    let error = message(
        call_project(
            &mut socket,
            5,
            "prepareSession",
            json!({"draftId": draft, "reuseDraft": true}),
        )
        .await,
    );
    assert!(error.contains("project.directory-required"), "{error}");
    assert!(!draft_path.exists());

    // 空项目：prepareSession 建默认目录并原子写入主 root。
    let root = dir.path().join("project-root");
    std::fs::create_dir_all(&root).unwrap();
    let Wire::Object(project) = call_project(
        &mut socket,
        6,
        "saveProject",
        json!({"defaultName": "Empty", "roots": [root.to_string_lossy()]}),
    )
    .await
    else {
        panic!("saveProject object")
    };
    let project_id = project["id"].as_str().unwrap().to_owned();
    let Wire::Object(prepared) = call_project(
        &mut socket,
        7,
        "prepareSession",
        json!({"projectId": project_id}),
    )
    .await
    else {
        panic!("prepareSession object")
    };
    assert_eq!(prepared["workspacePath"], json!(root.to_string_lossy()));
    assert_eq!(prepared["projectId"], json!(project_id));
    assert_eq!(prepared["additionalDirectories"], json!([]));

    // 项目删除后不再返回旧归属。
    call_project(&mut socket, 8, "deleteProject", json!(project_id)).await;
    let error = message(
        call_project(
            &mut socket,
            9,
            "prepareSession",
            json!({"projectId": project_id}),
        )
        .await,
    );
    assert!(error.contains("project.not-found"), "{error}");
}

#[tokio::test]
async fn read_directory_and_save_project_reject_invalid_paths() {
    let (dir, _surface, mut socket, _data_base_dir) = start_project_host().await;
    let home = dir.path().join("home");
    std::fs::write(home.join("visible.txt"), b"x").unwrap();
    std::fs::write(home.join(".hidden"), b"x").unwrap();

    let Wire::Object(entries) = call_project(&mut socket, 1, "readDirectory", json!({})).await
    else {
        panic!("readDirectory object")
    };
    assert_eq!(entries["path"], json!(home.to_string_lossy()));
    let names: Vec<&str> = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"visible.txt"));
    assert!(!names.contains(&".hidden"), "默认不含隐藏项: {names:?}");

    let Wire::Object(hidden) = call_project(
        &mut socket,
        2,
        "readDirectory",
        json!({"includeHidden": true}),
    )
    .await
    else {
        panic!("readDirectory object")
    };
    let names: Vec<&str> = hidden["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&".hidden"), "{names:?}");

    let error =
        message(call_project(&mut socket, 3, "readDirectory", json!({"path": "relative"})).await);
    assert!(error.contains("project.absolute-path-required"), "{error}");

    let error = message(
        call_project(
            &mut socket,
            4,
            "saveProject",
            json!({"roots": ["relative/path"]}),
        )
        .await,
    );
    assert!(error.contains("project.absolute-path-required"), "{error}");

    let missing = dir.path().join("does-not-exist");
    let error = message(
        call_project(
            &mut socket,
            5,
            "saveProject",
            json!({"roots": [missing.to_string_lossy()]}),
        )
        .await,
    );
    assert!(error.contains("project.directory-required"), "{error}");

    let error = message(
        call_project(
            &mut socket,
            6,
            "saveProject",
            json!({"id": "a", "restoreProjectId": "b", "roots": []}),
        )
        .await,
    );
    assert!(error.contains("project.ambiguous-save"), "{error}");

    // 主目录状态每次读取重新派生；删除 root 后变为 missing。
    let root = dir.path().join("tracked-root");
    std::fs::create_dir_all(&root).unwrap();
    let Wire::Object(saved) = call_project(
        &mut socket,
        7,
        "saveProject",
        json!({"name": "Tracked", "roots": [root.to_string_lossy()]}),
    )
    .await
    else {
        panic!("saveProject object")
    };
    let project_id = saved["id"].as_str().unwrap().to_owned();
    let Wire::Array(projects) = call_project(&mut socket, 8, "readProjects", json!({})).await
    else {
        panic!("readProjects array")
    };
    let tracked = projects
        .into_iter()
        .map(to_value)
        .find(|project| project["id"] == json!(project_id))
        .expect("saved project listed");
    assert_eq!(tracked["primaryRootStatus"], json!("available"));
    std::fs::remove_dir_all(&root).unwrap();
    let Wire::Array(projects) = call_project(&mut socket, 9, "readProjects", json!({})).await
    else {
        panic!("readProjects array")
    };
    let tracked = projects
        .into_iter()
        .map(to_value)
        .find(|project| project["id"] == json!(project_id))
        .expect("saved project listed");
    assert_eq!(tracked["primaryRootStatus"], json!("missing"));
}

#[tokio::test]
async fn moving_a_task_requires_an_existing_root_confirmation() {
    let (dir, _surface, mut socket, data_base_dir) = start_project_host().await;
    let workspace = dir.path().join("task-workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let project_root = dir.path().join("other-root");
    std::fs::create_dir_all(&project_root).unwrap();
    let Wire::Object(project) = call_project(
        &mut socket,
        1,
        "saveProject",
        json!({"name": "Target", "roots": [project_root.to_string_lossy()]}),
    )
    .await
    else {
        panic!("saveProject object")
    };
    let project_id = project["id"].as_str().unwrap().to_owned();

    let index_path = data_base_dir.join(".escode/v2/tasks-index.sqlite");
    let connection = rusqlite::Connection::open(&index_path).unwrap();
    connection
        .execute(
            "INSERT INTO tasks (
               workspace_key, workspace_path, task_id, title, mode, provider, created_at, updated_at,
               host_id, root_path, cli_created_at, cli_updated_at
             ) VALUES (?1, ?1, 'task_move', '移动', 'build', 'glm', 100, 200, 'local', ?1, 100, 200)",
            rusqlite::params![workspace.to_string_lossy()],
        )
        .unwrap();
    drop(connection);

    let task = json!({"workspacePath": workspace.to_string_lossy(), "taskId": "task_move"});
    let error = message(
        call_project(
            &mut socket,
            2,
            "moveTask",
            json!({"task": task, "projectId": project_id}),
        )
        .await,
    );
    assert!(
        error.contains("project.root-confirmation-required"),
        "{error}"
    );

    let missing = dir.path().join("missing-root");
    let error = message(
        call_project(
            &mut socket,
            3,
            "moveTask",
            json!({
                "task": {"workspacePath": missing.to_string_lossy(), "taskId": "task_move"},
                "projectId": project_id,
                "addMissingRoot": true,
            }),
        )
        .await,
    );
    assert!(error.contains("project.directory-required"), "{error}");

    call_project(
        &mut socket,
        4,
        "moveTask",
        json!({"task": task, "projectId": project_id, "addMissingRoot": true}),
    )
    .await;
    let connection = rusqlite::Connection::open(&index_path).unwrap();
    let moved: String = connection
        .query_row(
            "SELECT project_id FROM tasks WHERE workspace_key=?1 AND task_id='task_move'",
            rusqlite::params![workspace.to_string_lossy()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(moved, project_id);
    let roots: i64 = connection
        .query_row(
            "SELECT count(*) FROM project_roots WHERE project_id=?1",
            rusqlite::params![project_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(roots, 2, "缺 root 确认后原子加入");
}
