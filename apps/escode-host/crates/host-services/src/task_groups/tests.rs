//! 任务分组组织态：登记幂等、默认 cron 组、移动/解组墓碑（docs/specs/rust-project-storage.md §3.3）。

use rusqlite::Connection;
use serde_json::{Value, json};

use super::admit::admit;
use super::node_move::move_node;
use super::repository::{CRON_GROUP_ID, read_structure, save, ungroup};
use crate::tasks_index::schema;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    for statement in schema::MIGRATED_DDL {
        connection.execute_batch(statement).unwrap();
    }
    connection
}

fn insert_task(conn: &Connection, key: &str, task_id: &str, created_at: i64, cron: Option<&str>) {
    conn.execute(
        "INSERT INTO tasks(workspace_key,workspace_path,task_id,title,mode,created_at,updated_at,
           host_id,root_path,cli_created_at,cli_updated_at,cron_automation_id)
         VALUES(?1,?1,?2,'任务','build',?3,?3,'local',?1,?3,?3,?4)",
        rusqlite::params![key, task_id, created_at, cron],
    )
    .unwrap();
}

fn admission(conn: &Connection, key: &str, task_id: &str) -> (i64, Option<String>, Option<String>) {
    conn.query_row(
        "SELECT user_organized,system_kind,creation_id FROM task_group_admissions WHERE workspace_key=? AND task_id=?",
        rusqlite::params![key, task_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

fn order_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM task_group_view_node_orders",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn admit_registers_once_and_never_reorders_on_later_activity() {
    let conn = connection();
    insert_task(&conn, "/w", "t1", 10, None);
    let facts =
        json!([{"hostId": "local", "workspacePath": "/w", "taskId": "t1", "createdAt": 10}]);
    admit(&conn, &json!({"tasks": facts}), 1000).unwrap();
    assert_eq!(admission(&conn, "/w", "t1"), (0, None, None));
    assert_eq!(order_count(&conn), 1);
    // 第二次活动更新（createdAt 变大）不新增顶层位置。
    admit(
        &conn,
        &json!({"tasks": [{"hostId": "local", "workspacePath": "/w", "taskId": "t1", "createdAt": 20}]}),
        2000,
    )
    .unwrap();
    assert_eq!(order_count(&conn), 1);
}

#[test]
fn admit_puts_first_seen_cron_tasks_into_the_default_group() {
    let conn = connection();
    insert_task(&conn, "/w", "cron-1", 10, Some("cron-1"));
    admit(
        &conn,
        &json!({"tasks": [{
            "hostId": "local", "workspacePath": "/w", "taskId": "cron-1",
            "createdAt": 10, "cronAutomationId": "cron-1"
        }]}),
        1000,
    )
    .unwrap();
    assert_eq!(
        admission(&conn, "/w", "cron-1"),
        (0, Some("cron".to_owned()), None)
    );
    let (groups, members, _) = read_structure(&conn).unwrap();
    assert!(groups.iter().any(|group| group.id == CRON_GROUP_ID));
    assert!(
        members
            .iter()
            .any(|member| member.group_id == CRON_GROUP_ID && member.task_id == "cron-1")
    );
}

#[test]
fn ungroup_restores_the_top_level_order_and_keeps_the_organized_tombstone() {
    let conn = connection();
    insert_task(&conn, "/w", "t1", 10, None);
    let group = save(&conn, &json!({"title": "G"}), 1000).unwrap();
    let group_id = group["id"].as_str().unwrap();
    move_node(
        &conn,
        &json!({
            "node": {"type": "task", "task": {"hostId": "local", "workspacePath": "/w", "taskId": "t1"}},
            "sourceGroupId": null,
            "groupId": group_id,
        }),
        true,
        2000,
    )
    .unwrap();
    let (_, members, _) = read_structure(&conn).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(
        admission(&conn, "/w", "t1").0,
        1,
        "手工拖动登记 user_organized"
    );

    ungroup(&conn, group_id, 3000).unwrap();
    let (groups, members, order) = read_structure(&conn).unwrap();
    assert!(groups.is_empty());
    assert!(members.is_empty());
    assert!(
        order
            .iter()
            .any(|node| node.node_type == "task" && node.key.contains("t1"))
    );
    assert_eq!(admission(&conn, "/w", "t1").0, 1, "解组保留组织墓碑");
}

#[test]
fn move_rejects_stale_source_and_anchor() {
    let conn = connection();
    insert_task(&conn, "/w", "t1", 10, None);
    let group = save(&conn, &json!({"title": "G"}), 1000).unwrap();
    let group_id = group["id"].as_str().unwrap();
    let task = json!({"hostId": "local", "workspacePath": "/w", "taskId": "t1"});

    let stale = move_node(
        &conn,
        &json!({
            "node": {"type": "task", "task": task},
            "sourceGroupId": "another-group",
            "groupId": group_id,
        }),
        true,
        2000,
    );
    assert!(stale.is_err());
    assert!(
        stale
            .unwrap_err()
            .to_string()
            .contains("task-group.stale-source")
    );

    let stale_anchor = move_node(
        &conn,
        &json!({
            "node": {"type": "task", "task": task},
            "sourceGroupId": null,
            "groupId": group_id,
            "anchor": {"node": {"type": "task", "task": {"hostId": "local", "workspacePath": "/w", "taskId": "missing"}}, "side": "before"},
        }),
        true,
        2000,
    );
    assert!(stale_anchor.is_err());
    assert!(
        stale_anchor
            .unwrap_err()
            .to_string()
            .contains("task-group.stale-anchor")
    );
}

#[test]
fn deleted_tasks_leave_the_group_and_keep_the_tombstone() {
    let conn = connection();
    insert_task(&conn, "/w", "t1", 10, None);
    admit(
        &conn,
        &json!({"tasks": [{"hostId": "local", "workspacePath": "/w", "taskId": "t1", "createdAt": 10}]}),
        1000,
    )
    .unwrap();
    admit(
        &conn,
        &json!({"tasks": [{
            "hostId": "local", "workspacePath": "/w", "taskId": "t1",
            "createdAt": 10, "deleted": true
        }]}),
        2000,
    )
    .unwrap();
    let (_, members, order) = read_structure(&conn).unwrap();
    assert!(members.is_empty());
    assert!(!order.iter().any(|node| node.key.contains("t1")));
    assert_eq!(admission(&conn, "/w", "t1").0, 1);
    // 后续缺少 deleted 的投影不能重新入组。
    let before = order_count(&conn);
    admit(
        &conn,
        &json!({"tasks": [{"hostId": "local", "workspacePath": "/w", "taskId": "t1", "createdAt": 10}]}),
        3000,
    )
    .unwrap();
    assert_eq!(order_count(&conn), before);
}

#[test]
fn read_projects_the_structure_shape() {
    let conn = connection();
    let group = save(&conn, &json!({"title": " 标题 ", "color": "red"}), 1000).unwrap();
    let structure: Value = super::admit::read(&conn).unwrap();
    assert_eq!(structure["groups"][0]["id"], group["id"]);
    assert_eq!(structure["groups"][0]["title"], json!("标题"));
    assert_eq!(structure["groups"][0]["color"], json!("red"));
    assert!(
        structure["order"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["type"] == "group" && node["key"] == group["id"])
    );
}
