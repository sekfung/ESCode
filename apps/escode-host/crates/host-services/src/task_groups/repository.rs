//! 分组结构与事件序的纯 SQL 实现；事务由调用方（`api.rs`）按方法边界开启。

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashSet;
use uuid::Uuid;

use crate::tasks_index::workspace_key;

pub(super) const CRON_GROUP_ID: &str = "escode-default-group-cron";
const COLORS: [&str; 7] = ["gray", "red", "orange", "yellow", "green", "blue", "purple"];

#[derive(Debug, Clone)]
pub(super) struct Group {
    pub id: String,
    pub title: String,
    pub color: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub(super) struct Member {
    pub group_id: String,
    pub workspace_key: String,
    pub task_id: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone)]
pub(super) struct Ordered {
    pub node_type: String,
    pub key: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone)]
pub(super) struct Node {
    pub node_type: String,
    pub key: String,
}

/// 上游 `groupTaskKey`：`JSON.stringify([workspaceKey, taskId])`。
pub fn group_task_key(task: &Value) -> String {
    let key = workspace_key(
        task["workspacePath"].as_str().unwrap_or_default(),
        task["workspaceIdentity"].as_str(),
    );
    json!([key, task["taskId"].as_str().unwrap_or_default()]).to_string()
}

fn split_key(key: &str) -> (String, String) {
    let parsed: Value = serde_json::from_str(key).unwrap_or(Value::Null);
    (
        parsed[0].as_str().unwrap_or_default().to_owned(),
        parsed[1].as_str().unwrap_or_default().to_owned(),
    )
}

pub(super) fn read_structure(conn: &Connection) -> Result<(Vec<Group>, Vec<Member>, Vec<Ordered>)> {
    let mut groups = Vec::new();
    let mut statement = conn.prepare(
        "SELECT group_id,title,coalesce(color,'gray'),created_at,updated_at FROM task_groups ORDER BY created_at,group_id",
    )?;
    for row in statement.query_map([], |row| {
        Ok(Group {
            id: row.get(0)?,
            title: row.get(1)?,
            color: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })? {
        groups.push(row?);
    }
    let mut members = Vec::new();
    let mut statement = conn.prepare(
        "SELECT group_id,workspace_key,task_id,coalesce(sort_order,-added_at) AS sort_order FROM task_group_members ORDER BY sort_order,workspace_key,task_id",
    )?;
    for row in statement.query_map([], |row| {
        Ok(Member {
            group_id: row.get(0)?,
            workspace_key: row.get(1)?,
            task_id: row.get(2)?,
            sort_order: row.get(3)?,
        })
    })? {
        members.push(row?);
    }
    let mut order = Vec::new();
    let mut statement = conn.prepare(
        "SELECT node_type,node_key,sort_order FROM task_group_view_node_orders ORDER BY sort_order,node_type,node_key",
    )?;
    for row in statement.query_map([], |row| {
        Ok(Ordered {
            node_type: row.get(0)?,
            key: row.get(1)?,
            sort_order: row.get(2)?,
        })
    })? {
        order.push(row?);
    }
    Ok((groups, members, order))
}

pub(super) fn has_group(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM task_groups WHERE group_id=?", [id], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}

pub(super) fn parent(conn: &Connection, task: &Value) -> Result<Option<String>> {
    let key = workspace_key(
        task["workspacePath"].as_str().unwrap_or_default(),
        task["workspaceIdentity"].as_str(),
    );
    let task_id = task["taskId"].as_str().unwrap_or_default();
    Ok(conn
        .query_row(
            "SELECT group_id FROM task_group_members WHERE workspace_key=? AND task_id=?",
            params![key, task_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

/// 顶层（`groupId=None`）或组内成员的有序节点，补上尚未进入 order 表的组。
pub(super) fn children(conn: &Connection, group_id: Option<&str>) -> Result<Vec<Node>> {
    let (groups, members, order) = read_structure(conn)?;
    if let Some(group_id) = group_id {
        return Ok(members
            .into_iter()
            .filter(|member| member.group_id == group_id)
            .map(|member| Node {
                node_type: "task".to_owned(),
                key: json!([member.workspace_key, member.task_id]).to_string(),
            })
            .collect());
    }
    let member_keys: HashSet<String> = members
        .iter()
        .map(|member| json!([member.workspace_key, member.task_id]).to_string())
        .collect();
    let group_ids: HashSet<&str> = groups.iter().map(|group| group.id.as_str()).collect();
    let mut nodes: Vec<Node> = order
        .iter()
        .filter(|node| {
            if node.node_type == "group" {
                group_ids.contains(node.key.as_str())
            } else {
                !member_keys.contains(&node.key)
            }
        })
        .map(|node| Node {
            node_type: node.node_type.clone(),
            key: node.key.clone(),
        })
        .collect();
    let ordered_groups: HashSet<String> = nodes
        .iter()
        .filter(|node| node.node_type == "group")
        .map(|node| node.key.clone())
        .collect();
    nodes.extend(
        groups
            .iter()
            .filter(|group| !ordered_groups.contains(group.id.as_str()))
            .map(|group| Node {
                node_type: "group".to_owned(),
                key: group.id.clone(),
            }),
    );
    Ok(nodes)
}

pub(super) fn write_order(
    conn: &Connection,
    nodes: &[Node],
    group_id: Option<&str>,
    now: i64,
) -> Result<()> {
    for (position, node) in nodes.iter().enumerate() {
        if let Some(group_id) = group_id {
            let (workspace_key, task_id) = split_key(&node.key);
            conn.execute(
                "UPDATE task_group_members SET sort_order=?,updated_at=? WHERE group_id=? AND workspace_key=? AND task_id=?",
                params![position as i64, now, group_id, workspace_key, task_id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO task_group_view_node_orders(node_type,node_key,sort_order,created_at,updated_at)
                 VALUES(?,?,?,?,?) ON CONFLICT(node_type,node_key) DO UPDATE SET sort_order=excluded.sort_order,updated_at=excluded.updated_at",
                params![node.node_type, node.key, position as i64, now, now],
            )?;
        }
    }
    Ok(())
}

pub(super) fn mark_organized(conn: &Connection, workspace_key: &str, task_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO task_group_admissions(workspace_key,task_id,user_organized) VALUES(?,?,1)
         ON CONFLICT(workspace_key,task_id) DO UPDATE SET user_organized=1",
        params![workspace_key, task_id],
    )?;
    Ok(())
}

pub(crate) fn save(conn: &Connection, input: &Value, now: i64) -> Result<Value> {
    let id_input = input["id"].as_str().filter(|value| !value.is_empty());
    let (groups, _, _) = read_structure(conn)?;
    let previous = id_input.and_then(|id| groups.iter().find(|group| group.id == id).cloned());
    if id_input.is_some() && previous.is_none() {
        bail!("task-group.not-found");
    }
    let title = input["title"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| previous.as_ref().map(|group| group.title.clone()))
        .unwrap_or_else(|| "New group".to_owned());
    if title.is_empty() {
        bail!("task-group.title-required");
    }
    let color = input["color"]
        .as_str()
        .or_else(|| previous.as_ref().map(|group| group.color.as_str()))
        .unwrap_or("gray")
        .to_owned();
    if !COLORS.contains(&color.as_str()) {
        bail!("task-group.invalid-color");
    }
    let group = Group {
        id: previous
            .as_ref()
            .map_or_else(|| Uuid::new_v4().to_string(), |group| group.id.clone()),
        title,
        color,
        created_at: previous.as_ref().map_or(now, |group| group.created_at),
        updated_at: now,
    };
    conn.execute(
        "INSERT INTO task_groups(group_id,title,color,created_at,updated_at) VALUES(?,?,?,?,?)
         ON CONFLICT(group_id) DO UPDATE SET title=excluded.title,color=excluded.color,updated_at=excluded.updated_at",
        params![group.id, group.title, group.color, group.created_at, group.updated_at],
    )?;
    if previous.is_none() {
        let mut nodes = vec![Node {
            node_type: "group".to_owned(),
            key: group.id.clone(),
        }];
        nodes.extend(
            children(conn, None)?
                .into_iter()
                .filter(|node| node.key != group.id),
        );
        write_order(conn, &nodes, None, now)?;
    }
    Ok(json!({
        "id": group.id,
        "title": group.title,
        "color": group.color,
        "createdAt": group.created_at,
        "updatedAt": group.updated_at,
    }))
}

pub(crate) fn ungroup(conn: &Connection, id: &str, now: i64) -> Result<()> {
    if !has_group(conn, id)? {
        return Ok(());
    }
    let members = children(conn, Some(id))?;
    let mut root = Vec::new();
    for node in children(conn, None)? {
        if node.node_type == "group" && node.key == id {
            root.extend(members.iter().cloned());
        } else {
            root.push(node);
        }
    }
    for member in &members {
        let (workspace_key, task_id) = split_key(&member.key);
        mark_organized(conn, &workspace_key, &task_id)?;
    }
    conn.execute("DELETE FROM task_group_members WHERE group_id=?", [id])?;
    conn.execute("DELETE FROM task_groups WHERE group_id=?", [id])?;
    conn.execute(
        "DELETE FROM task_group_view_node_orders WHERE node_type='group' AND node_key=?",
        [id],
    )?;
    write_order(conn, &root, None, now)
}

/// 任务事实（`GroupTaskFact`）：分组只消费 CLI/索引事实，不推导运行态。
pub fn read_group_task_fact(conn: &Connection, task: &Value) -> Result<Option<Value>> {
    let key = workspace_key(
        task["workspacePath"].as_str().unwrap_or_default(),
        task["workspaceIdentity"].as_str(),
    );
    let task_id = task["taskId"].as_str().unwrap_or_default();
    let fact = conn
        .query_row(
            "SELECT workspace_path,workspace_identity,cron_automation_id,
                    coalesce(cli_created_at,created_at) AS created_at,
                    archived,deleted,coalesce(cli_deleted,0)
             FROM tasks WHERE workspace_key=? AND task_id=?",
            params![key, task_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((workspace_path, identity, cron, created_at, archived, deleted, cli_deleted)) = fact
    else {
        return Ok(None);
    };
    let mut value = json!({
        "hostId": "local",
        "workspacePath": workspace_path,
        "taskId": task_id,
        "createdAt": created_at,
        "archived": archived == 1,
        "deleted": deleted == 1 || cli_deleted == 1,
    });
    if let Some(identity) = identity.filter(|value| !value.trim().is_empty()) {
        value["workspaceIdentity"] = json!(identity);
    }
    if let Some(cron) = cron.filter(|value| !value.is_empty()) {
        value["cronAutomationId"] = json!(cron);
    }
    Ok(Some(value))
}
