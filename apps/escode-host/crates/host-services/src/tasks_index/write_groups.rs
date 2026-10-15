//! tasks-index 的**分组视图写**（TS `TaskIndexRepo.createTaskGroup` / `renameTaskGroup` / `updateTaskGroupColor` /
//! `deleteTaskGroup` / `applyGroupedTaskViewOrder`，`packages/services/src/session/taskIndexRepo.ts:2260-2920`）。
//!
//! 规则（不能简化）：
//! - 新建 group 立即进入顶层排序并插到最顶（`MIN(sort_order) - STEP`；不能依赖 `created_at` 与已有 `sort_order` 混排）；
//! - 删除 group 与其顶层排序行同一事务提交；
//! - `applyGroupedTaskViewOrder` 先校验（scope 内、可见、同一 task 不得进入多个 group、group 必须存在；带 provider 时
//!   旧 provider 的 task 引用作为遗留数据静默跳过），再在**一个事务**里提交 membership + 全量顶层排序，
//!   并写全局 bootstrap marker（此后查询不再做 workspace 自动分组初始化）。

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use super::TasksIndexError;
use super::query_shared::GROUP_COLORS;
use super::write_state::transaction;
use crate::git::paths::js_trim;

/// TS `GROUPED_TASK_ORDER_STEP`。
pub(super) const ORDER_STEP: i64 = 1000;
/// TS `GROUPED_WORKSPACE_BOOTSTRAP_ONCE_KEY`。
pub(super) const BOOTSTRAP_ONCE_KEY: &str = "__escode_internal_grouped_workspace_bootstrap_once__";
const DEFAULT_COLOR: &str = "gray";
const DEFAULT_TITLE: &str = "New Group";

pub(super) fn rejected(message: &str) -> TasksIndexError {
    TasksIndexError::Rejected(message.to_owned())
}

pub(super) fn sqlite(error: rusqlite::Error) -> TasksIndexError {
    TasksIndexError::Sqlite(error)
}

fn group_json(id: &str, title: &str, color: &str, created_at: i64, updated_at: i64) -> Value {
    json!({
        "id": id,
        "title": title,
        "color": if GROUP_COLORS.contains(&color) { color } else { DEFAULT_COLOR },
        "createdAt": created_at,
        "updatedAt": updated_at,
    })
}

/// TS `taskOrderNodeKey`：`JSON.stringify([workspaceKey, taskId])`（不用 NUL 分隔：SQLite TEXT 会截断）。
pub fn task_order_node_key(workspace_key: &str, task_id: &str) -> String {
    serde_json::to_string(&[workspace_key, task_id]).unwrap_or_default()
}

/// TS `getNextGroupedTopSortOrder`。
fn next_top_sort_order(connection: &Connection) -> Result<i64, TasksIndexError> {
    let min: Option<i64> = connection
        .query_row(
            "SELECT MIN(sort_order) FROM task_group_view_node_orders",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite)?;
    Ok(min.unwrap_or(ORDER_STEP * 2) - ORDER_STEP)
}

/// TS `upsertGroupedTopOrder`。
fn upsert_top_order(
    connection: &Connection,
    node_type: &str,
    node_key: &str,
    sort_order: i64,
    now: i64,
) -> Result<(), TasksIndexError> {
    connection
        .execute(
            "INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(node_type, node_key) DO UPDATE SET
               sort_order = excluded.sort_order, updated_at = excluded.updated_at",
            params![node_type, node_key, sort_order, now],
        )
        .map_err(sqlite)?;
    Ok(())
}

/// TS `createTaskGroup`（`id` 与 `now` 由调用方给，便于确定性测试）。颜色不校验（TS 同）。
pub fn create_task_group(
    connection: &Connection,
    id: &str,
    title: Option<&str>,
    color: Option<&str>,
    now: i64,
) -> Result<Value, TasksIndexError> {
    let title = title
        .map(js_trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_TITLE);
    let color = color.unwrap_or(DEFAULT_COLOR);
    // TS 两条语句不在同一事务；这里合并成一个事务（崩溃不会留下没有顶层排序的 group，且对调用方不可见）。
    transaction(connection, || {
        connection
            .execute(
                "INSERT INTO task_groups (group_id, title, color, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![id, title, color, now],
            )
            .map_err(sqlite)?;
        let sort_order = next_top_sort_order(connection)?;
        upsert_top_order(connection, "group", id, sort_order, now)
    })?;
    // TS 回包不经 `rowToTaskGroup` 的颜色校验：原样回传传入的颜色。
    Ok(json!({"id": id, "title": title, "color": color, "createdAt": now, "updatedAt": now}))
}

fn read_group(connection: &Connection, id: &str) -> Result<Option<Value>, TasksIndexError> {
    connection
        .query_row(
            "SELECT group_id, title, color, created_at, updated_at FROM task_groups WHERE group_id = ?1",
            params![id],
            |row| {
                Ok(group_json(
                    &row.get::<_, String>(0)?,
                    &row.get::<_, String>(1)?,
                    &row.get::<_, String>(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite)
}

/// TS `renameTaskGroup`。
pub fn rename_task_group(
    connection: &Connection,
    id: &str,
    title: &str,
    now: i64,
) -> Result<Value, TasksIndexError> {
    let title = Some(js_trim(title))
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_TITLE);
    let changes = connection
        .execute(
            "UPDATE task_groups SET title = ?1, updated_at = ?2 WHERE group_id = ?3",
            params![title, now, id],
        )
        .map_err(sqlite)?;
    if changes == 0 {
        return Err(rejected("Task group 不存在，无法重命名"));
    }
    read_group(connection, id)?.ok_or_else(|| rejected("Task group 重命名后读取失败"))
}

/// TS `updateTaskGroupColor`。
pub fn update_task_group_color(
    connection: &Connection,
    id: &str,
    color: &str,
    now: i64,
) -> Result<Value, TasksIndexError> {
    if !GROUP_COLORS.contains(&color) {
        return Err(rejected("Task group 颜色无效"));
    }
    let changes = connection
        .execute(
            "UPDATE task_groups SET color = ?1, updated_at = ?2 WHERE group_id = ?3",
            params![color, now, id],
        )
        .map_err(sqlite)?;
    if changes == 0 {
        return Err(rejected("Task group 不存在，无法更新颜色"));
    }
    read_group(connection, id)?.ok_or_else(|| rejected("Task group 更新颜色后读取失败"))
}

/// TS `deleteTaskGroup`。
pub fn delete_task_group(connection: &Connection, id: &str) -> Result<(), TasksIndexError> {
    transaction(connection, || {
        let changes = connection
            .execute("DELETE FROM task_groups WHERE group_id = ?1", params![id])
            .map_err(sqlite)?;
        if changes == 0 {
            return Err(rejected("Task group 不存在，无法删除"));
        }
        connection
            .execute(
                "DELETE FROM task_group_view_node_orders WHERE node_type = 'group' AND node_key = ?1",
                params![id],
            )
            .map_err(sqlite)?;
        Ok(())
    })
}

/// 供测试与调用方：把 group 成员读成 `group_id → [(workspace_key, task_id)]`（按 sort_order）。
pub fn read_members(
    connection: &Connection,
) -> Result<BTreeMap<String, Vec<(String, String)>>, TasksIndexError> {
    let mut statement = connection
        .prepare("SELECT group_id, workspace_key, task_id FROM task_group_members ORDER BY group_id, sort_order, task_id")
        .map_err(sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(sqlite)?;
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for row in rows {
        let (group, key, task) = row.map_err(sqlite)?;
        out.entry(group).or_default().push((key, task));
    }
    Ok(out)
}

pub use super::write_groups_apply::{
    GroupInput, OrderInput, TaskRefInput, TopNode, apply_grouped_task_view_order,
};
