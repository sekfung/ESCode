//! 项目/任务组织态的 SQLite 读写（对齐上游 `project-storage/project-repository.ts`）。
//!
//! Rust Host 只服务本地 workspace（远程仍由 Node Host，见 Q3），因此 ownerHostId 固定 `local`；
//! 远端 single-root 约束由 Node Host 负责。表结构来自生成资产 `schema.rs`。

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

pub(super) const LOCAL_HOST: &str = "local";
const AGENT_PROVIDER: &str = "glm";
pub(super) const OWNERSHIP_DECIDED: i64 = 8;

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(super) fn workspace_key(task: &Value) -> String {
    task["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| task["workspacePath"].as_str().unwrap_or_default())
        .to_string()
}

pub(super) fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

pub(super) fn roots_of(value: &Value) -> Vec<String> {
    value["roots"]
        .as_array()
        .map(|roots| {
            roots
                .iter()
                .filter_map(|root| root.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn transaction<T>(
    conn: &Connection,
    write: bool,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    conn.execute_batch(if write { "BEGIN IMMEDIATE" } else { "BEGIN" })?;
    match body() {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn project_roots(conn: &Connection) -> Result<Vec<(String, Value)>> {
    let mut statement = conn.prepare(
        "SELECT project_id, root_path, position, is_primary FROM project_roots ORDER BY position, root_path",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            json!({
                "rootPath": row.get::<_, String>(1)?,
                "position": row.get::<_, i64>(2)?,
                "isPrimary": row.get::<_, i64>(3)? == 1,
            }),
        ))
    })?;
    let mut roots = Vec::new();
    for row in rows {
        roots.push(row?);
    }
    Ok(roots)
}

pub fn read_projects(conn: &Connection, include_deleted: bool) -> Result<Vec<Value>> {
    let roots = project_roots(conn)?;
    let sql = if include_deleted {
        "SELECT project_id, host_id, name, name_overridden, position, pinned, deleted, created_at, updated_at FROM projects ORDER BY position, project_id"
    } else {
        "SELECT project_id, host_id, name, name_overridden, position, pinned, deleted, created_at, updated_at FROM projects WHERE deleted=0 ORDER BY position, project_id"
    };
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        let id: String = row.get(0)?;
        let project_roots = roots
            .iter()
            .filter(|(project_id, _)| project_id == &id)
            .map(|(_, root)| root.clone())
            .collect::<Vec<_>>();
        Ok(json!({
            "id": id,
            "hostId": row.get::<_, String>(1)?,
            "name": row.get::<_, String>(2)?,
            "nameOverridden": row.get::<_, i64>(3)? == 1,
            "position": row.get::<_, i64>(4)?,
            "pinned": row.get::<_, i64>(5)? == 1,
            "deleted": row.get::<_, i64>(6)? == 1,
            "createdAt": row.get::<_, i64>(7)?,
            "updatedAt": row.get::<_, i64>(8)?,
            "roots": project_roots,
        }))
    })?;
    let mut projects = Vec::new();
    for row in rows {
        projects.push(row?);
    }
    Ok(projects)
}

pub fn require_project(conn: &Connection, id: &str) -> Result<Value> {
    read_projects(conn, true)?
        .into_iter()
        .find(|project| project["id"] == id)
        .ok_or_else(|| anyhow::anyhow!("project.not-found"))
}

pub(super) fn remote_identity(value: &str) -> bool {
    value.starts_with("remote:")
}

pub(super) fn read_task(conn: &Connection, task: &Value) -> Result<Option<Map<String, Value>>> {
    let mut statement = conn.prepare(
        "SELECT workspace_key, workspace_path, workspace_identity, task_id, host_id, root_path, project_id, execution_directories, title, title_overridden, provider, mode, model, pinned, archived, unread_at, deleted, meta_json, cli_title, cli_title_source, cli_created_at, cli_updated_at, cli_last_outcome, cli_deleted FROM tasks WHERE workspace_key=? AND task_id=?",
    )?;
    let mut rows = statement.query(params![workspace_key(task), text(task, "taskId")])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let mut map = Map::new();
    for (index, name) in [
        "workspaceKey",
        "workspacePath",
        "workspaceIdentity",
        "taskId",
        "hostId",
        "rootPath",
        "projectId",
        "executionDirectories",
        "title",
        "titleOverridden",
        "provider",
        "mode",
        "model",
        "pinned",
        "archived",
        "unreadAt",
        "deleted",
        "metaJson",
        "cliTitle",
        "cliTitleSource",
        "cliCreatedAt",
        "cliUpdatedAt",
        "cliLastOutcome",
        "cliDeleted",
    ]
    .iter()
    .enumerate()
    {
        let value: rusqlite::types::Value = row.get(index)?;
        let value = match value {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(number) => json!(number),
            rusqlite::types::Value::Real(number) => json!(number),
            rusqlite::types::Value::Text(text) => json!(text),
            rusqlite::types::Value::Blob(bytes) => json!(bytes),
        };
        map.insert((*name).to_string(), value);
    }
    Ok(Some(map))
}

fn parse_directories(value: &Value) -> Result<Vec<String>> {
    let Some(text) = value.as_str().filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let parsed: Value = serde_json::from_str(text)?;
    let Some(array) = parsed.as_array() else {
        bail!("project.invalid-execution-directories");
    };
    if array.len() > 256 || array.iter().any(|entry| entry.as_str().is_none()) {
        bail!("project.invalid-execution-directories");
    }
    Ok(array
        .iter()
        .filter_map(|entry| entry.as_str().map(str::to_owned))
        .collect())
}

pub(super) fn resolve_directories(conn: &Connection, task: &Value) -> Result<Value> {
    let row = read_task(conn, task)?;
    let workspace_path = row
        .as_ref()
        .and_then(|row| row.get("rootPath"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| task["workspacePath"].as_str().unwrap_or_default())
        .to_string();
    let project = match row
        .as_ref()
        .and_then(|row| row.get("projectId"))
        .and_then(Value::as_str)
    {
        Some(id) => Some(require_project(conn, id)?),
        None => None,
    };
    let execution_directories = row
        .as_ref()
        .and_then(|row| row.get("executionDirectories"))
        .map(parse_directories)
        .transpose()?
        .unwrap_or_default();
    let additional = if execution_directories.is_empty() {
        project
            .as_ref()
            .and_then(|project| project["roots"].as_array())
            .map(|roots| {
                roots
                    .iter()
                    .filter_map(|root| root["rootPath"].as_str())
                    .filter(|root| *root != workspace_path)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        execution_directories
    };
    Ok(json!({
        "workspacePath": workspace_path,
        "projectId": project.as_ref().and_then(|project| project["id"].as_str()),
        "additionalDirectories": additional,
    }))
}

fn read_tasks(conn: &Connection) -> Result<Vec<Value>> {
    let mut statement = conn.prepare(
        "SELECT * FROM tasks WHERE provider=? AND deleted=0 AND coalesce(cli_deleted,0)=0 AND cli_created_at IS NOT NULL ORDER BY cli_updated_at DESC, task_id",
    )?;
    let mut rows = statement.query([AGENT_PROVIDER])?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next()? {
        let meta_json: Option<String> = row.get("meta_json").ok();
        let mut meta = meta_json
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let task_id: String = row.get("task_id")?;
        let workspace_key: String = row.get("workspace_key")?;
        let workspace_identity: Option<String> = row.get("workspace_identity").ok();
        let root_path: String = row.get("root_path")?;
        let title: String = row.get("title")?;
        let title_overridden: i64 = row.get("title_overridden")?;
        let cli_title: Option<String> = row.get("cli_title").ok();
        let cli_title_source: Option<String> = row.get("cli_title_source").ok();
        let cli_last_outcome: Option<String> = row.get("cli_last_outcome").ok();
        let pinned: i64 = row.get("pinned")?;
        let archived: i64 = row.get("archived")?;
        let unread_at: Option<i64> = row.get("unread_at").ok();
        let project_id: Option<String> = row.get("project_id").ok();
        let host_id: String = row.get("host_id")?;
        let created_at: Option<i64> = row.get("cli_created_at").ok();
        let updated_at: Option<i64> = row.get("cli_updated_at").ok();
        meta["taskId"] = json!(task_id);
        meta["workspacePath"] = json!(root_path);
        meta["workspaceIdentity"] = if workspace_identity.as_deref() == Some(workspace_key.as_str())
        {
            json!(workspace_identity)
        } else if remote_identity(&workspace_key) {
            json!(workspace_key)
        } else {
            Value::Null
        };
        meta["title"] = json!(if title_overridden == 1 {
            title
        } else {
            cli_title.unwrap_or_default()
        });
        meta["titleOverridden"] =
            json!(title_overridden == 1 || cli_title_source.as_deref() == Some("custom"));
        meta["createdAt"] = json!(created_at);
        meta["updatedAt"] = json!(updated_at);
        meta["status"] = match cli_last_outcome.as_deref() {
            Some("error") => json!("error"),
            Some(_) => json!("completed"),
            None => Value::Null,
        };
        meta["mode"] = meta
            .get("mode")
            .cloned()
            .unwrap_or_else(|| json!("default"));
        meta["pinned"] = json!(pinned == 1);
        meta["archived"] = json!(archived == 1);
        meta["unreadAt"] = json!(unread_at);
        meta["projectId"] = json!(project_id);
        meta["hostId"] = json!(host_id);
        tasks.push(meta);
    }
    Ok(tasks)
}

pub fn read_tree(
    conn: &Connection,
    include_deleted: bool,
    include_deleted_tasks: bool,
    include_tasks: bool,
) -> Result<Value> {
    let read = || -> Result<Value> {
        let projects = read_projects(conn, include_deleted)?
            .into_iter()
            .filter(|project| project["hostId"] == LOCAL_HOST)
            .collect::<Vec<_>>();
        let deleted_tasks = if include_deleted_tasks {
            let mut statement = conn.prepare(
                "SELECT task_id, workspace_path, workspace_identity, host_id FROM tasks WHERE host_id=? AND (deleted=1 OR coalesce(cli_deleted,0)=1)",
            )?;
            let rows = statement.query_map([LOCAL_HOST], |row| {
                Ok(json!({
                    "taskId": row.get::<_, String>(0)?,
                    "workspacePath": row.get::<_, String>(1)?,
                    "workspaceIdentity": row.get::<_, Option<String>>(2)?,
                    "hostId": row.get::<_, String>(3)?,
                }))
            })?;
            let mut values = Vec::new();
            for row in rows {
                values.push(row?);
            }
            Some(values)
        } else {
            None
        };
        let tasks = if include_tasks {
            read_tasks(conn)?
                .into_iter()
                .filter(|task| task["hostId"] == LOCAL_HOST)
                .collect()
        } else {
            Vec::new()
        };
        let mut tree = json!({"projects": projects, "tasks": tasks});
        if let Some(deleted_tasks) = deleted_tasks {
            tree["deletedTasks"] = json!(deleted_tasks);
        }
        Ok(tree)
    };
    if conn.is_autocommit() {
        transaction(conn, false, read)
    } else {
        read()
    }
}
