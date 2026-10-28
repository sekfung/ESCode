//! `admit`：任务列表首次发现时的分组登记（上游 `TaskGroupRepository.admit`）。
//!
//! 首次发现只写一个顶层位置，避免大库启动时逐条重排造成平方级写放大；已登记节点不因活动更新重排。

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use super::node_move::move_node;
use super::repository::{
    CRON_GROUP_ID, group_task_key, mark_organized, parent, read_structure, write_order,
};
use super::repository::{Node, children};
use crate::tasks_index::workspace_key;

fn has_group(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM task_groups WHERE group_id=?", [id], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}

fn ensure_cron_group(conn: &Connection, now: i64) -> Result<()> {
    if has_group(conn, CRON_GROUP_ID)? {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO task_groups(group_id,title,color,created_at,updated_at) VALUES(?,?,?,?,?)",
        params![CRON_GROUP_ID, "cron", "blue", now, now],
    )?;
    let mut nodes = vec![Node {
        node_type: "group".to_owned(),
        key: CRON_GROUP_ID.to_owned(),
    }];
    nodes.extend(
        children(conn, None)?
            .into_iter()
            .filter(|node| node.key != CRON_GROUP_ID),
    );
    write_order(conn, &nodes, None, now)
}

pub(crate) fn admit(conn: &Connection, input: &Value, now: i64) -> Result<()> {
    let mut facts: Vec<Value> = input["tasks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(Value::is_object)
        .collect();
    // 批量首次发现按创建时间从旧到新写入顶部；stable sort 保持同值原始顺序。
    facts.sort_by_key(|task| task["createdAt"].as_i64().unwrap_or(0));
    let placement = input.get("placement").filter(|value| !value.is_null());
    let placement_key = placement.map(|placement| group_task_key(&placement["task"]));
    for task in &facts {
        let key = workspace_key(
            task["workspacePath"].as_str().unwrap_or_default(),
            task["workspaceIdentity"].as_str(),
        );
        let task_key = group_task_key(task);
        let task_id = task["taskId"].as_str().unwrap_or_default();
        if task["deleted"] == true {
            conn.execute(
                "DELETE FROM task_group_members WHERE workspace_key=? AND task_id=?",
                params![key, task_id],
            )?;
            conn.execute(
                "DELETE FROM task_group_view_node_orders WHERE node_type='task' AND node_key=?",
                [&task_key],
            )?;
            // 删除事实可能晚到；保留登记墓碑，后续缺少 deleted 的投影不能重新入组。
            mark_organized(conn, &key, task_id)?;
            continue;
        }
        let mut admission = conn
            .query_row(
                "SELECT system_kind,user_organized,creation_id FROM task_group_admissions WHERE workspace_key=? AND task_id=?",
                params![key, task_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        if admission.is_none() {
            let legacy = parent(conn, task)?.is_some()
                || conn
                    .query_row(
                        "SELECT 1 FROM task_group_view_node_orders WHERE node_type='task' AND node_key=?",
                        [&task_key],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
            conn.execute(
                "INSERT INTO task_group_admissions(workspace_key,task_id,user_organized) VALUES(?,?,?)",
                params![key, task_id, i64::from(legacy)],
            )?;
            admission = Some((None, i64::from(legacy), None));
            if !legacy {
                let order: i64 = conn.query_row(
                    "SELECT coalesce(min(sort_order),0)-1 FROM task_group_view_node_orders",
                    [],
                    |row| row.get(0),
                )?;
                conn.execute(
                    "INSERT INTO task_group_view_node_orders(node_type,node_key,sort_order,created_at,updated_at) VALUES('task',?,?,?,?)",
                    params![task_key, order, now, now],
                )?;
            }
        }
        let (system_kind, user_organized, creation_id) = admission.expect("admission ensured");
        let kind = task["cronAutomationId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(|_| "cron");
        if let Some(kind) = kind
            && system_kind.is_none()
        {
            conn.execute(
                "UPDATE task_group_admissions SET system_kind=? WHERE workspace_key=? AND task_id=?",
                params![kind, key, task_id],
            )?;
            // 首次类型确认才入组；不能把读列表变成重复覆盖用户整理的「自愈」。
            if user_organized == 0 && task["archived"] != true && parent(conn, task)?.is_none() {
                ensure_cron_group(conn, now)?;
                move_node(
                    conn,
                    &json!({
                        "node": {"type": "task", "task": task},
                        "sourceGroupId": Value::Null,
                        "groupId": CRON_GROUP_ID,
                    }),
                    false,
                    now,
                )?;
            }
        }
        if placement_key.as_deref() == Some(task_key.as_str()) && creation_id.is_none() {
            if user_organized == 0 {
                let group_id = match placement
                    .and_then(|placement| placement["groupId"].as_str())
                    .filter(|value| !value.is_empty())
                {
                    Some(id) if has_group(conn, id)? => Some(id),
                    _ => None,
                };
                let source = parent(conn, task)?;
                move_node(
                    conn,
                    &json!({
                        "node": {"type": "task", "task": task},
                        "sourceGroupId": source,
                        "groupId": group_id,
                    }),
                    true,
                    now,
                )?;
            }
            if let Some(placement) = placement {
                conn.execute(
                    "UPDATE task_group_admissions SET creation_id=? WHERE workspace_key=? AND task_id=?",
                    params![
                        placement["creationId"].as_str().unwrap_or_default(),
                        key,
                        task_id
                    ],
                )?;
            }
        }
    }
    Ok(())
}

/// `read` 的快照投影（groups/members/order）。
pub(crate) fn read(conn: &Connection) -> Result<Value> {
    let (groups, members, order) = read_structure(conn)?;
    Ok(json!({
        "groups": groups
            .iter()
            .map(|group| json!({
                "id": group.id,
                "title": group.title,
                "color": group.color,
                "createdAt": group.created_at,
                "updatedAt": group.updated_at,
            }))
            .collect::<Vec<_>>(),
        "members": members
            .iter()
            .map(|member| json!({
                "groupId": member.group_id,
                "workspaceKey": member.workspace_key,
                "taskId": member.task_id,
                "sortOrder": member.sort_order,
            }))
            .collect::<Vec<_>>(),
        "order": order
            .iter()
            .map(|node| json!({
                "type": node.node_type,
                "key": node.key,
                "sortOrder": node.sort_order,
            }))
            .collect::<Vec<_>>(),
    }))
}
