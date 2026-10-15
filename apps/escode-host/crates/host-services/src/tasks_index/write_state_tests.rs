//! `write_state` 的语义边界（对齐 TS `TaskIndexRepo.updateTaskState` / `clearTaskUnreadIfMatches` /
//! `deleteArchivedTask` 的每条规则；跨实现 oracle 见 `packages/desktop/test/rustHostRpc.test.ts`）。
use super::test_support::{expected_ledger, fixture};
use super::write_state::{Field, StatePatch, TaskRef};
use super::*;
use serde_json::json;

fn open() -> (tempfile::TempDir, TasksIndex) {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).unwrap();
    let meta = json!({"taskId": "t1", "traceId": "escode-t1", "title": "原标题", "workspacePath": "/w",
        "createdAt": 10, "updatedAt": 100, "mode": "build"});
    index
        .sync_task_meta(meta.as_object().unwrap(), &SyncOptions::default())
        .unwrap();
    (dir, index)
}

fn task() -> TaskRef<'static> {
    TaskRef {
        workspace_path: "/w",
        workspace_identity: None,
        task_id: "t1",
    }
}

fn unread(at: i64) -> StatePatch {
    StatePatch {
        unread_at: Field::Set(at),
        ..StatePatch::default()
    }
}

#[test]
fn unread_markers_are_strictly_increasing_even_across_clears() {
    let (_dir, index) = open();
    let first = index.update_task_state(&task(), &unread(1_000)).unwrap();
    assert_eq!(first["unreadAt"], 1_000);
    // 同毫秒再次未读：版本必须前进，否则 CAS 清除会误命中旧点击。
    let second = index.update_task_state(&task(), &unread(1_000)).unwrap();
    assert_eq!(second["unreadAt"], 1_001);
    let cleared = index
        .update_task_state(
            &task(),
            &StatePatch {
                unread_at: Field::Clear,
                ..StatePatch::default()
            },
        )
        .unwrap();
    assert!(cleared.get("unreadAt").is_none(), "{cleared}");
    // 清除后 watermark 不重置：仍从 1001 之后分配。
    let third = index.update_task_state(&task(), &unread(1_000)).unwrap();
    assert_eq!(third["unreadAt"], 1_002);
    // 更大的请求值直接采用。
    let fourth = index.update_task_state(&task(), &unread(5_000)).unwrap();
    assert_eq!(fourth["unreadAt"], 5_000);
}

#[test]
fn clear_unread_is_a_compare_and_swap() {
    let (_dir, index) = open();
    let marked = index.update_task_state(&task(), &unread(2_000)).unwrap();
    let marker = marked["unreadAt"].as_i64().unwrap();
    // 旧点击（期望值过期）不能清掉新未读。
    let (meta, cleared) = index
        .clear_task_unread_if_matches(&task(), marker - 1)
        .unwrap();
    assert!(!cleared);
    assert_eq!(meta["unreadAt"], marker);
    let (meta, cleared) = index.clear_task_unread_if_matches(&task(), marker).unwrap();
    assert!(cleared);
    assert!(meta.get("unreadAt").is_none());
}

#[test]
fn rename_pin_and_flags_keep_the_rest_of_the_row() {
    let (_dir, index) = open();
    let renamed = index
        .update_task_state(
            &task(),
            &StatePatch {
                title: Some("手动标题".into()),
                title_overridden: Some(true),
                updated_at: Some(300),
                pinned: Some(true),
                ..StatePatch::default()
            },
        )
        .unwrap();
    assert_eq!(renamed["title"], "手动标题");
    assert_eq!(renamed["titleOverridden"], true);
    assert_eq!(renamed["updatedAt"], 300);
    let pinned: i64 = index
        .connection_for_tests()
        .query_row("SELECT pinned FROM tasks WHERE task_id = 't1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(pinned, 1);
    // agent 同步不能冲掉用户标题（title_overridden 生效）。
    let agent = json!({"taskId": "t1", "traceId": "escode-t1", "title": "agent 标题", "workspacePath": "/w",
        "createdAt": 10, "updatedAt": 400, "mode": "build"});
    let synced = index
        .sync_task_meta(agent.as_object().unwrap(), &SyncOptions::default())
        .unwrap();
    assert_eq!(synced["title"], "手动标题");
}

#[test]
fn delete_tombstones_the_row_and_its_grouping_references_atomically() {
    let (dir, index) = open();
    let connection = rusqlite::Connection::open(dir.path().join("tasks-index.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
             VALUES ('task', ?1, 1, 0, 0)",
            [r#"["/w","t1"]"#],
        )
        .unwrap();
    index
        .update_task_state(
            &task(),
            &StatePatch {
                deleted: Some(true),
                ..StatePatch::default()
            },
        )
        .unwrap();
    let orders: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM task_group_view_node_orders",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orders, 0, "grouping references deleted with the tombstone");
    assert_eq!(index.get_task_meta("/w", None, "t1").unwrap(), None);
    // 已删除的任务再写状态：TS 同文错误。
    let error = index.update_task_state(&task(), &unread(1)).unwrap_err();
    assert_eq!(error.to_string(), "task index 中不存在 task: t1");
}

#[test]
fn delete_archived_task_only_removes_archived_rows() {
    let (_dir, index) = open();
    assert_eq!(
        index.delete_archived_task(&task()).unwrap(),
        None,
        "not archived: skipped"
    );
    index
        .update_task_state(
            &task(),
            &StatePatch {
                archived: Some(true),
                ..StatePatch::default()
            },
        )
        .unwrap();
    let deleted = index
        .delete_archived_task(&task())
        .unwrap()
        .expect("archived row deleted");
    assert_eq!(deleted["taskId"], "t1");
    assert_eq!(index.get_task_meta("/w", None, "t1").unwrap(), None);
    assert_eq!(
        index.delete_archived_task(&task()).unwrap(),
        None,
        "already deleted: skipped"
    );
}
