//! 分组节点的移动（上游 `TaskGroupRepository.moveInside`）：来源校验、锚点解析与有序写回。

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::repository::{
    Node, children, group_task_key, has_group, mark_organized, parent, write_order,
};
use crate::tasks_index::workspace_key;

fn node_of(node: &Value) -> Node {
    if node["type"] == "group" {
        Node {
            node_type: "group".to_owned(),
            key: node["groupId"].as_str().unwrap_or_default().to_owned(),
        }
    } else {
        Node {
            node_type: "task".to_owned(),
            key: group_task_key(&node["task"]),
        }
    }
}

pub(crate) fn move_node(conn: &Connection, input: &Value, manual: bool, now: i64) -> Result<()> {
    let node = node_of(&input["node"]);
    let group_id = input["groupId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let source_group_id = input["sourceGroupId"]
        .as_str()
        .filter(|value| !value.is_empty());
    if input["node"]["type"] == "task" {
        let parent = parent(conn, &input["node"]["task"])?;
        if parent.as_deref() != source_group_id {
            bail!("task-group.stale-source");
        }
    }
    if node.node_type == "group" && (group_id.is_some() || !has_group(conn, &node.key)?) {
        bail!("task-group.invalid-group");
    }
    if let Some(group_id) = &group_id
        && !has_group(conn, group_id)?
    {
        bail!("task-group.not-found");
    }
    let mut destination: Vec<Node> = children(conn, group_id.as_deref())?
        .into_iter()
        .filter(|candidate| !(candidate.node_type == node.node_type && candidate.key == node.key))
        .collect();
    let index = match input.get("anchor").filter(|anchor| !anchor.is_null()) {
        Some(anchor) => {
            let anchor = node_of(&anchor["node"]);
            destination
                .iter()
                .position(|candidate| {
                    candidate.node_type == anchor.node_type && candidate.key == anchor.key
                })
                .ok_or_else(|| anyhow::anyhow!("task-group.stale-anchor"))?
        }
        None => 0,
    };
    let after = input["anchor"]["side"].as_str() == Some("after");
    destination.insert(index + usize::from(after), node.clone());
    if node.node_type == "task" {
        let task = &input["node"]["task"];
        let key = workspace_key(
            task["workspacePath"].as_str().unwrap_or_default(),
            task["workspaceIdentity"].as_str(),
        );
        let task_id = task["taskId"].as_str().unwrap_or_default();
        if manual {
            mark_organized(conn, &key, task_id)?;
        }
        conn.execute(
            "DELETE FROM task_group_members WHERE workspace_key=? AND task_id=?",
            params![key, task_id],
        )?;
        if let Some(group_id) = &group_id {
            conn.execute(
                "INSERT INTO task_group_members(group_id,workspace_key,workspace_path,workspace_identity,task_id,sort_order,added_at,created_at,updated_at)
                 VALUES(?,?,?,?,?,0,?,?,?)",
                params![
                    group_id,
                    key,
                    task["workspacePath"].as_str().unwrap_or_default(),
                    task["workspaceIdentity"].as_str(),
                    task_id,
                    now,
                    now,
                    now
                ],
            )?;
        }
    }
    write_order(conn, &destination, group_id.as_deref(), now)
}
