//! 项目/任务组织态的写面（上游 `project-repository.ts` 的 save/delete/reorder/move/reserve）。

use super::project_read::{
    LOCAL_HOST, OWNERSHIP_DECIDED, now_ms, read_task, require_project, resolve_directories,
    roots_of, text, transaction, workspace_key,
};
use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

/// 写面 host：缺省本地；远端 owner 必须显式给出 `hostId`。
fn host_id_of(input: &Value) -> &str {
    input["hostId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(LOCAL_HOST)
}

/// `projects.host_id` 有外键；首次写入远端项目时登记 `server` host 行。
fn ensure_host(conn: &Connection, host_id: &str) -> Result<()> {
    let kind = if host_id == LOCAL_HOST {
        "local"
    } else {
        "server"
    };
    conn.execute(
        "INSERT OR IGNORE INTO hosts(host_id,kind,position) VALUES(?,?,(SELECT count(*) FROM hosts))",
        params![host_id, kind],
    )?;
    Ok(())
}

pub(super) fn save_project(conn: &Connection, input: &Value) -> Result<Value> {
    transaction(conn, true, || {
        let requested_host = host_id_of(input);
        ensure_host(conn, requested_host)?;
        let id_input = input["id"].as_str().filter(|value| !value.is_empty());
        let restore = input["restoreProjectId"]
            .as_str()
            .filter(|value| !value.is_empty());
        let old = match (id_input, restore) {
            (Some(id), _) | (None, Some(id)) => Some(require_project(conn, id)?),
            (None, None) => None,
        };
        if id_input.is_some() && old.as_ref().is_some_and(|old| old["deleted"] == true) {
            bail!("project.deleted");
        }
        // 客户端 hostId 不能改变已有项目的持库身份（远端与本地项目不能互相搬库）。
        if let Some(old) = old.as_ref()
            && old["hostId"].as_str() != Some(requested_host)
        {
            bail!("project.host-immutable");
        }
        let host_id = old
            .as_ref()
            .and_then(|old| old["hostId"].as_str())
            .unwrap_or(requested_host)
            .to_owned();
        let mut roots = roots_of(input);
        if roots.is_empty() {
            roots = old
                .as_ref()
                .and_then(|old| old["roots"].as_array())
                .map(|roots| {
                    roots
                        .iter()
                        .filter_map(|root| root["rootPath"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
        }
        let mut seen = HashSet::new();
        roots.retain(|root| seen.insert(root.clone()));
        // 远端 owner 只允许单一固定 root（上游 project-storage/CONTRACT.md）。
        if host_id != LOCAL_HOST && roots.len() != 1 {
            bail!("project.remote-single-root-required");
        }
        if let Some(restore) = restore {
            let old = old
                .as_ref()
                .filter(|old| old["deleted"] == true)
                .ok_or_else(|| anyhow::anyhow!("project.not-found"))?;
            let old_roots = old["roots"].as_array().cloned().unwrap_or_default();
            if roots.len() != old_roots.len()
                || roots.iter().any(|root| {
                    !old_roots
                        .iter()
                        .any(|entry| entry["rootPath"].as_str() == Some(root.as_str()))
                })
            {
                bail!("project.restore-roots-mismatch");
            }
            let _ = restore;
        }
        let primary = input["primaryRoot"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                old.as_ref()
                    .and_then(|old| old["roots"].as_array())
                    .and_then(|roots| {
                        roots.iter().find(|root| {
                            root["isPrimary"] == true
                                && root["rootPath"].as_str().is_some_and(|path| {
                                    roots
                                        .iter()
                                        .any(|entry| entry["rootPath"].as_str() == Some(path))
                                })
                        })
                    })
                    .and_then(|root| root["rootPath"].as_str().map(str::to_owned))
            })
            .or_else(|| roots.first().cloned());
        if primary
            .as_deref()
            .is_some_and(|primary| !roots.iter().any(|root| root == primary))
        {
            bail!("project.invalid-primary-root");
        }
        let id = old
            .as_ref()
            .and_then(|old| old["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let overridden = input["name"]
            .as_str()
            .map(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                old.as_ref()
                    .is_some_and(|old| old["nameOverridden"] == true)
            });
        let base_name = input["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                overridden
                    .then(|| old.as_ref().and_then(|old| old["name"].as_str()))
                    .flatten()
                    .map(str::to_owned)
            })
            .or_else(|| {
                roots.first().map(|root| {
                    Path::new(root)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(root)
                        .to_string()
                })
            })
            .or_else(|| {
                input["defaultName"]
                    .as_str()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .or_else(|| {
                old.as_ref()
                    .and_then(|old| old["name"].as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "New project".to_string());
        let mut name = base_name.clone();
        let mut suffix = 2;
        loop {
            let taken: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE host_id=? AND name=? AND deleted=0 AND project_id<>?)",
                params![host_id, name, id],
                |row| row.get(0),
            )?;
            if !taken {
                break;
            }
            name = format!("{base_name} ({suffix})");
            suffix += 1;
        }
        let now = now_ms();
        let pinned = input["pinned"]
            .as_bool()
            .or_else(|| old.as_ref().and_then(|old| old["pinned"].as_bool()))
            .unwrap_or(false);
        conn.execute(
            "INSERT INTO projects(project_id,host_id,name,name_overridden,position,pinned,pinned_at,created_at,updated_at)
             VALUES(?,?,?,?,(SELECT coalesce(min(position),1)-1 FROM projects WHERE host_id=?),?,?,?,?)
             ON CONFLICT(project_id) DO UPDATE SET name=excluded.name,name_overridden=excluded.name_overridden,
             pinned=excluded.pinned,pinned_at=excluded.pinned_at,deleted=0,deleted_at=NULL,updated_at=excluded.updated_at",
            params![
                id,
                host_id,
                name,
                i64::from(overridden),
                host_id,
                i64::from(pinned),
                if pinned { Some(now) } else { None },
                now,
                now
            ],
        )?;
        conn.execute("DELETE FROM project_roots WHERE project_id=?", [&id])?;
        for (position, root) in roots.iter().enumerate() {
            conn.execute(
                "INSERT INTO project_roots(project_id,root_path,position,is_primary) VALUES(?,?,?,?)",
                params![
                    id,
                    root,
                    position as i64,
                    i64::from(primary.as_deref() == Some(root.as_str()))
                ],
            )?;
        }
        require_project(conn, &id)
    })
}

/// 上游 `initializeProjectRoot`：只在项目仍无 roots 时写入主 root（事务内空判断）。
pub(super) fn initialize_project_root(conn: &Connection, input: &Value) -> Result<Value> {
    let id = input["id"].as_str().unwrap_or_default();
    let path = input["path"].as_str().unwrap_or_default();
    transaction(conn, true, || {
        let project = require_project(conn, id)?;
        if project["hostId"] != LOCAL_HOST {
            bail!("project.cross-host-move");
        }
        if project["deleted"] == true {
            bail!("project.deleted");
        }
        let empty = project["roots"].as_array().is_none_or(Vec::is_empty);
        if empty {
            conn.execute(
                "INSERT INTO project_roots(project_id,root_path,position,is_primary) VALUES(?,?,0,1)",
                params![id, path],
            )?;
            conn.execute(
                "UPDATE projects SET updated_at=? WHERE project_id=?",
                params![now_ms(), id],
            )?;
        }
        require_project(conn, id)
    })
}

pub(super) fn delete_project(conn: &Connection, input: &Value) -> Result<()> {
    let id = input["id"].as_str().unwrap_or_default();
    let host_id = host_id_of(input);
    let now = now_ms();
    conn.execute(
        "UPDATE projects SET deleted=1,deleted_at=?,updated_at=? WHERE project_id=? AND deleted=0 AND host_id=?",
        params![now, now, id, host_id],
    )?;
    Ok(())
}

pub(super) fn reorder_projects(conn: &Connection, input: &Value) -> Result<()> {
    let ids = input["ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        bail!("project.duplicate-order");
    }
    let host_id = host_id_of(input).to_owned();
    transaction(conn, true, || {
        for (position, id) in ids.iter().enumerate() {
            if require_project(conn, id)?["hostId"] != json!(host_id) {
                bail!("project.cross-host-move");
            }
            conn.execute(
                "UPDATE projects SET position=? WHERE project_id=?",
                params![position as i64, id],
            )?;
        }
        Ok(())
    })
}

pub(super) fn move_task(conn: &Connection, input: &Value) -> Result<()> {
    let task = &input["task"];
    let project_id = input["projectId"].as_str().unwrap_or_default();
    transaction(conn, true, || {
        let project = require_project(conn, project_id)?;
        if project["deleted"] == true {
            bail!("project.deleted");
        }
        let project_host = project["hostId"].as_str().unwrap_or(LOCAL_HOST);
        if input["hostId"]
            .as_str()
            .is_some_and(|host| !host.is_empty() && host != project_host)
        {
            bail!("project.cross-host-move");
        }
        let Some(row) = read_task(conn, task)? else {
            bail!("project.task-not-found");
        };
        let task_host = row
            .get("hostId")
            .and_then(Value::as_str)
            .unwrap_or(LOCAL_HOST);
        if task_host != project_host {
            bail!("project.cross-host-move");
        }
        let root_path = row
            .get("rootPath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let roots = project["roots"].as_array().cloned().unwrap_or_default();
        if !roots
            .iter()
            .any(|root| root["rootPath"].as_str() == Some(root_path.as_str()))
        {
            if input["addMissingRoot"] != true {
                bail!("project.root-confirmation-required");
            }
            conn.execute(
                "INSERT INTO project_roots(project_id,root_path,position,is_primary) VALUES(?,?,?,?)",
                params![
                    project_id,
                    root_path,
                    roots.len() as i64,
                    i64::from(roots.is_empty())
                ],
            )?;
        }
        conn.execute(
            "UPDATE tasks SET project_id=?,handoff_pending=handoff_pending | ? WHERE workspace_key=? AND task_id=?",
            params![
                project_id,
                OWNERSHIP_DECIDED,
                workspace_key(task),
                text(task, "taskId")
            ],
        )?;
        Ok(())
    })
}

pub(super) fn resolve_session_directories(conn: &Connection, task: &Value) -> Result<Value> {
    resolve_directories(conn, task)
}

pub(super) fn reserve_created_task(conn: &Connection, input: &Value) -> Result<Value> {
    let task = &input["task"];
    let host_id = task["hostId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(LOCAL_HOST)
        .to_owned();
    transaction(conn, true, || {
        let key = workspace_key(task);
        ensure_host(conn, &host_id)?;
        if read_task(conn, task)?.is_none() {
            let project_id = input["projectId"]
                .as_str()
                .filter(|value| !value.is_empty());
            let additional = input["additionalDirectories"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            conn.execute(
                "INSERT INTO tasks(workspace_key,workspace_path,workspace_identity,task_id,created_at,updated_at,host_id,root_path,project_id,handoff_pending,execution_directories)
                 VALUES(?,?,?,?,0,0,?,?,?,?,?)",
                params![
                    key,
                    text(task, "workspacePath"),
                    task["workspaceIdentity"].as_str(),
                    text(task, "taskId"),
                    host_id,
                    text(task, "workspacePath"),
                    project_id,
                    OWNERSHIP_DECIDED,
                    if additional.is_empty() { None } else { Some(json!(additional).to_string()) }
                ],
            )?;
        }
        resolve_directories(conn, task)
    })
}
