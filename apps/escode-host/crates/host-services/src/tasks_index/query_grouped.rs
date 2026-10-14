//! TS `TaskIndexRepo.queryGroupedTaskViewStructure`：group / 成员 / 顶层排序原始结构。
//! 与 `query_task_list` 同库同连接（只读），从 `query.rs` 拆出以守住单文件行数上限。

use rusqlite::Connection;
use serde_json::{Value, json};

use super::TasksIndexError;
use super::query_shared::{GROUP_COLORS, resolve_member_identity};
use super::workspace_key;

/// TS `queryGroupedTaskViewStructure`：group（bootstrap 可见性过滤）+ 全量成员 + 顶层排序。
pub(super) fn query_grouped_task_view_structure(
    connection: &Connection,
    workspace_scopes: &[(&str, Option<&str>)],
) -> Result<Value, TasksIndexError> {
    let visible_keys: std::collections::BTreeSet<String> = workspace_scopes
        .iter()
        .map(|(path, identity)| workspace_key(path, *identity))
        .filter(|key| !key.trim().is_empty())
        .collect();

    // bootstrap 组只在其 workspace 可见（`group_id IS NOT NULL` 的行是 workspace 自举组）。
    let mut bootstrap_statement = connection
        .prepare(
            "SELECT workspace_key, group_id FROM task_group_workspace_bootstraps
             WHERE group_id IS NOT NULL",
        )
        .map_err(TasksIndexError::Sqlite)?;
    let bootstrap_rows = bootstrap_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(TasksIndexError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(TasksIndexError::Sqlite)?;
    let bootstrap_by_group: std::collections::HashMap<&str, &str> = bootstrap_rows
        .iter()
        .map(|(workspace_key, group_id)| (group_id.as_str(), workspace_key.as_str()))
        .collect();

    let mut group_statement = connection
        .prepare("SELECT group_id, title, color, created_at, updated_at FROM task_groups")
        .map_err(TasksIndexError::Sqlite)?;
    let group_rows = group_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(TasksIndexError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(TasksIndexError::Sqlite)?;
    let groups: Vec<Value> = group_rows
        .iter()
        .filter(|(group_id, ..)| {
            bootstrap_by_group
                .get(group_id.as_str())
                .is_none_or(|key| visible_keys.contains(*key))
        })
        .map(|(group_id, title, color, created_at, updated_at)| {
            json!({
                "id": group_id,
                "title": title,
                "color": if GROUP_COLORS.contains(&color.as_str()) {
                    color.clone()
                } else {
                    "gray".to_owned()
                },
                "createdAt": created_at,
                "updatedAt": updated_at,
            })
        })
        .collect();

    let mut member_statement = connection
        .prepare(
            "SELECT group_id, workspace_key, workspace_path, workspace_identity, task_id,
                    sort_order, added_at
             FROM task_group_members",
        )
        .map_err(TasksIndexError::Sqlite)?;
    let member_rows = member_statement
        .query_map([], |row| {
            Ok(json!({
                "groupId": row.get::<_, String>(0)?,
                "workspaceKey": row.get::<_, String>(1)?,
                "workspacePath": row.get::<_, String>(2)?,
                "workspaceIdentity": row.get::<_, Option<String>>(3)?,
                "taskId": row.get::<_, String>(4)?,
                "sortOrder": row.get::<_, Option<i64>>(5)?,
                "addedAt": row.get::<_, i64>(6)?,
            }))
        })
        .map_err(TasksIndexError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(TasksIndexError::Sqlite)?;
    // `workspaceIdentity` 按 TS 的行解析规则归一（可能与 key 一致或丢弃）。
    let members: Vec<Value> = member_rows
        .iter()
        .map(|member| {
            let mut member = member.clone();
            let key = member["workspaceKey"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let path = member["workspacePath"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let column = member["workspaceIdentity"].as_str().map(str::to_owned);
            let identity = resolve_member_identity(&key, &path, column.as_deref());
            match identity {
                Some(identity) => member["workspaceIdentity"] = json!(identity),
                None => {
                    member
                        .as_object_mut()
                        .expect("member is an object")
                        .remove("workspaceIdentity");
                }
            }
            if member["sortOrder"].is_null() {
                member
                    .as_object_mut()
                    .expect("member is an object")
                    .remove("sortOrder");
            }
            member
        })
        .collect();

    let mut order_statement = connection
        .prepare("SELECT node_type, node_key, sort_order FROM task_group_view_node_orders")
        .map_err(TasksIndexError::Sqlite)?;
    let order_rows = order_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(TasksIndexError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(TasksIndexError::Sqlite)?;
    let mut top_level_orders = Vec::new();
    for (node_type, node_key, sort_order) in order_rows {
        if node_type == "group" {
            top_level_orders.push(json!({
                "type": "group",
                "groupId": node_key,
                "sortOrder": sort_order,
            }));
            continue;
        }
        // task node_key = `JSON.stringify([workspaceKey, taskId])`；历史脏键跳过（TS 同）。
        let Ok(parsed) = serde_json::from_str::<Value>(&node_key) else {
            continue;
        };
        let Some(parts) = parsed.as_array() else {
            continue;
        };
        let (Some(first), Some(second)) = (parts.first(), parts.get(1)) else {
            continue;
        };
        let (Some(workspace_key), Some(task_id)) = (first.as_str(), second.as_str()) else {
            continue;
        };
        top_level_orders.push(json!({
            "type": "task",
            "workspaceKey": workspace_key,
            "taskId": task_id,
            "sortOrder": sort_order,
        }));
    }
    Ok(json!({
        "groups": groups,
        "members": members,
        "topLevelOrders": top_level_orders,
    }))
}
