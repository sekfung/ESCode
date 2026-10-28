//! 项目自动归档与测试辅助查询（上游 `project-repository.ts` 的 archiveInactive 等）。

use super::project_read::{LOCAL_HOST, remote_identity, text, transaction, workspace_key};
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::HashSet;

pub(super) fn archive_inactive(conn: &Connection, input: &Value) -> Result<Vec<Value>> {
    let cutoff = input["cutoff"].as_i64().unwrap_or(0);
    let active: HashSet<(String, String)> = input["activeTasks"]
        .as_array()
        .map(|tasks| {
            tasks
                .iter()
                .map(|task| (workspace_key(task), text(task, "taskId")))
                .collect()
        })
        .unwrap_or_default();
    transaction(conn, true, || {
        let mut statement = conn.prepare(
            "SELECT workspace_key,workspace_path,workspace_identity,task_id FROM tasks
             WHERE archived=0 AND pinned=0 AND deleted=0 AND coalesce(cli_deleted,0)=0
             AND cli_updated_at < ? AND host_id=?",
        )?;
        let rows = statement.query_map(params![cutoff, LOCAL_HOST], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(row?);
        }
        let mut archived = Vec::new();
        for (workspace_key, workspace_path, workspace_identity, task_id) in candidates {
            if active.contains(&(workspace_key.clone(), task_id.clone())) {
                continue;
            }
            conn.execute(
                "UPDATE tasks SET archived=1 WHERE workspace_key=? AND task_id=?",
                params![workspace_key, task_id],
            )?;
            let mut task = json!({"workspacePath": workspace_path, "taskId": task_id});
            if let Some(identity) = workspace_identity
                .filter(|identity| remote_identity(identity) || identity == &workspace_key)
            {
                task["workspaceIdentity"] = json!(identity);
            }
            archived.push(task);
        }
        Ok(archived)
    })
}
