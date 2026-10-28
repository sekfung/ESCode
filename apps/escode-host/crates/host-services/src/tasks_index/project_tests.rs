use super::test_support::fixture;
use super::*;
use serde_json::json;

fn index() -> (tempfile::TempDir, TasksIndex) {
    let dir = fixture(&super::test_support::expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect("open");
    index
        .connection
        .execute_batch(
            "INSERT INTO tasks(workspace_key,workspace_path,task_id,provider,created_at,updated_at,host_id,root_path,project_id,handoff_pending,execution_directories,meta_json,deleted,archived,pinned,title)\n\
             VALUES('/ws','/ws','t1','glm',0,0,'local','/ws',NULL,0,NULL,'{}',0,0,0,'');
             UPDATE tasks SET cli_created_at=0, cli_updated_at=0 WHERE task_id='t1';",
        )
        .unwrap();
    (dir, index)
}

#[test]
fn save_read_and_soft_delete_project() {
    let (_dir, index) = index();
    let saved = index
        .project_save(&json!({"name":"Demo","roots":["/ws","/other"],"primaryRoot":"/ws"}))
        .expect("save");
    assert_eq!(saved["name"], "Demo");
    assert_eq!(saved["roots"].as_array().unwrap().len(), 2);
    assert_eq!(saved["roots"][0]["isPrimary"], true);

    let projects = index.project_read_projects(false).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["id"], saved["id"]);

    index
        .project_delete(&json!({"id": saved["id"]}))
        .expect("delete");
    assert!(index.project_read_projects(false).unwrap().is_empty());
    assert_eq!(index.project_read_projects(true).unwrap().len(), 1);
}

#[test]
fn move_task_and_resolve_directories() {
    let (_dir, index) = index();
    let saved = index
        .project_save(&json!({"name":"Demo","roots":["/ws"],"primaryRoot":"/ws"}))
        .expect("save");
    index
        .project_move_task(&json!({
            "task":{"workspacePath":"/ws","taskId":"t1"},
            "projectId": saved["id"]
        }))
        .expect("move");
    let tree = index.project_read_tree(false, false, true).unwrap();
    assert_eq!(tree["tasks"][0]["projectId"], saved["id"]);
    let directories = index
        .project_resolve_directories(&json!({"workspacePath":"/ws","taskId":"t1"}))
        .expect("resolve");
    assert_eq!(directories["workspacePath"], "/ws");
    assert_eq!(directories["projectId"], saved["id"]);
    assert_eq!(directories["additionalDirectories"], json!([]));
}

#[test]
fn reorder_projects_rejects_duplicates_and_applies_order() {
    let (_dir, index) = index();
    let a = index
        .project_save(&json!({"name":"A","roots":["/a"]}))
        .unwrap();
    let b = index
        .project_save(&json!({"name":"B","roots":["/b"]}))
        .unwrap();
    index
        .project_reorder(&json!({"ids":[b["id"], a["id"]]}))
        .unwrap();
    let projects = index.project_read_projects(false).unwrap();
    assert_eq!(projects[0]["id"], b["id"]);
    assert_eq!(projects[1]["id"], a["id"]);
    assert!(
        index
            .project_reorder(&json!({"ids":[a["id"], a["id"]]}))
            .is_err()
    );
}

#[test]
fn reserve_created_task_records_ownership_before_first_send() {
    let (_dir, index) = index();
    let saved = index
        .project_save(&json!({"name":"Demo","roots":["/ws"]}))
        .unwrap();
    index.connection.execute("DELETE FROM tasks", []).unwrap();
    let directories = index
        .project_reserve_task(&json!({
            "task":{"workspacePath":"/ws","taskId":"t2"},
            "projectId": saved["id"]
        }))
        .expect("reserve");
    assert_eq!(directories["projectId"], saved["id"]);
    let tree = index.project_read_tree(false, false, true).unwrap();
    assert!(
        tree["tasks"].as_array().unwrap().is_empty(),
        "无 CLI 事实的预留行不进列表"
    );
}
