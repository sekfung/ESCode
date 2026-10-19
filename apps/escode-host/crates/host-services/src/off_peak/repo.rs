//! `off_peak_tasks` 本地仓库（TS `offPeakTaskRepo.ts` 的管理面子集）。
//!
//! 与 TS 共用 `tasks-index.sqlite` 与表结构；本模块只做 list/get/编辑/暂停/继续/取消/删除，
//! 不实现取号、轮询、派发认领与终态核销（那些依赖服务端/调度器）。

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

const TERMINAL: [&str; 3] = ["completed", "failed", "cancelled"];

#[derive(Clone)]
pub struct OffPeakRepo {
    path: std::path::PathBuf,
}

impl OffPeakRepo {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    fn open(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.path).map_err(|error| error.to_string())?;
        connection
            .busy_timeout(Duration::from_millis(5000))
            .map_err(|error| error.to_string())?;
        Ok(connection)
    }

    pub fn list(&self) -> Result<Vec<Value>, String> {
        let connection = self.open()?;
        let has_tasks = table_exists(&connection, "tasks")?;
        let sql = if has_tasks {
            "SELECT t.*, s.title AS session_title FROM off_peak_tasks t \
             LEFT JOIN tasks s ON s.workspace_key = t.workspace_key AND s.task_id = t.session_id \
             ORDER BY t.created_at DESC"
        } else {
            "SELECT * FROM off_peak_tasks ORDER BY created_at DESC"
        };
        let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], row_to_task)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(rows)
    }

    pub fn get(&self, id: &str) -> Result<Option<Value>, String> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT * FROM off_peak_tasks WHERE off_peak_task_id = ?1",
                [id],
                row_to_task,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let connection = self.open()?;
        connection
            .execute(
                "DELETE FROM off_peak_tasks WHERE off_peak_task_id = ?1",
                [id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn delete_history(&self, id: &str, now: i64) -> Result<Option<Value>, String> {
        let connection = self.open()?;
        connection
            .execute(
                "UPDATE off_peak_tasks SET history_deleted_at = ?2, updated_at = ?2 \
                 WHERE off_peak_task_id = ?1 AND started_at IS NOT NULL AND history_deleted_at IS NULL",
                rusqlite::params![id, now],
            )
            .map_err(|error| error.to_string())?;
        self.get(id)
    }

    pub fn set_paused(&self, id: &str, paused: bool, now: i64) -> Result<Option<Value>, String> {
        let connection = self.open()?;
        let (from, to) = if paused {
            ("queued", "paused")
        } else {
            ("paused", "queued")
        };
        let changed = connection
            .execute(
                "UPDATE off_peak_tasks SET status = ?2, updated_at = ?3 \
                 WHERE off_peak_task_id = ?1 AND status = ?4 AND claim_running = 0",
                rusqlite::params![id, to, now, from],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Ok(None);
        }
        self.get(id)
    }

    pub fn cancel(&self, id: &str, now: i64) -> Result<Option<Value>, String> {
        let Some(existing) = self.get(id)? else {
            return Ok(None);
        };
        if terminal(&existing) {
            return Ok(Some(existing));
        }
        let connection = self.open()?;
        connection
            .execute(
                "UPDATE off_peak_tasks SET status = 'cancelled', ended_at = ?2, updated_at = ?2 \
                 WHERE off_peak_task_id = ?1",
                rusqlite::params![id, now],
            )
            .map_err(|error| error.to_string())?;
        self.get(id)
    }

    pub fn update(&self, id: &str, params: &Value, now: i64) -> Result<Option<Value>, String> {
        let Some(existing) = self.get(id)? else {
            return Ok(None);
        };
        let status = existing["status"].as_str().unwrap_or_default();
        if status != "queued" && status != "paused" {
            return Ok(None);
        }
        if params.get("modelSelection").is_some_and(Value::is_null) {
            return Ok(None);
        }
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_else(|| existing["title"].as_str().unwrap_or_default());
        let prompt = params
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_else(|| existing["prompt"].as_str().unwrap_or_default());
        let permission_mode = params
            .get("permissionMode")
            .and_then(Value::as_str)
            .unwrap_or_else(|| existing["permissionMode"].as_str().unwrap_or_default());
        let selection = match params.get("modelSelection") {
            Some(value) if !value.is_null() => Some(value.clone()),
            _ => existing.get("modelSelection").cloned(),
        };
        let Some(selection) = selection else {
            return Ok(None);
        };
        let connection = self.open()?;
        connection
            .execute(
                "UPDATE off_peak_tasks SET title = ?2, prompt = ?3, permission_mode = ?4, \
                 model_selection = ?5, updated_at = ?6 WHERE off_peak_task_id = ?1",
                rusqlite::params![
                    id,
                    title,
                    prompt,
                    permission_mode,
                    serde_json::to_string(&selection).map_err(|error| error.to_string())?,
                    now
                ],
            )
            .map_err(|error| error.to_string())?;
        self.get(id)
    }
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(|error| error.to_string())
}

fn terminal(task: &Value) -> bool {
    task["status"]
        .as_str()
        .is_some_and(|status| TERMINAL.contains(&status))
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let id: String = row.get("off_peak_task_id")?;
    let status: String = row.get("status")?;
    let mut task = Map::new();
    task.insert("offPeakTaskId".to_owned(), Value::String(id));
    insert_optional(
        &mut task,
        "serverTicketId",
        row.get::<_, Option<String>>("server_ticket_id")?,
    );
    task.insert("title".to_owned(), Value::String(row.get("title")?));
    insert_optional(
        &mut task,
        "conversationId",
        row.get::<_, Option<String>>("conversation_id")?,
    );
    insert_optional(
        &mut task,
        "sessionId",
        row.get::<_, Option<String>>("session_id")?,
    );
    if let Ok(Some(title)) = row.get::<_, Option<String>>("session_title")
        && !title.is_empty()
    {
        task.insert("sessionTitle".to_owned(), Value::String(title));
    }
    task.insert("prompt".to_owned(), Value::String(row.get("prompt")?));
    task.insert(
        "permissionMode".to_owned(),
        Value::String(row.get("permission_mode")?),
    );
    let selection: Option<String> = row.get("model_selection")?;
    let parsed = selection
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(Value::is_object);
    let has_selection = parsed.is_some();
    match parsed {
        Some(selection) => {
            task.insert("modelSelection".to_owned(), selection);
        }
        None => {
            if !TERMINAL.contains(&status.as_str()) {
                let mut issue = Map::new();
                issue.insert(
                    "code".to_owned(),
                    Value::String("repair-required".to_owned()),
                );
                if let Ok(Some(model)) = row.get::<_, Option<String>>("model") {
                    issue.insert("legacyModelId".to_owned(), Value::String(model));
                }
                if let Ok(Some(level)) = row.get::<_, Option<String>>("thought_level") {
                    issue.insert("legacyReasoningLevel".to_owned(), Value::String(level));
                }
                task.insert("modelSelectionIssue".to_owned(), Value::Object(issue));
            }
        }
    }
    task.insert(
        "workspaceKey".to_owned(),
        Value::String(row.get("workspace_key")?),
    );
    task.insert(
        "workspacePath".to_owned(),
        Value::String(row.get("workspace_path")?),
    );
    insert_optional(
        &mut task,
        "workspaceIdentity",
        row.get::<_, Option<String>>("workspace_identity")?,
    );
    task.insert("status".to_owned(), Value::String(status.clone()));
    task.insert(
        "queuedAt".to_owned(),
        Value::from(row.get::<_, i64>("queued_at")?),
    );
    insert_optional(
        &mut task,
        "startedAt",
        row.get::<_, Option<i64>>("started_at")?,
    );
    insert_optional(&mut task, "endedAt", row.get::<_, Option<i64>>("ended_at")?);
    insert_optional(
        &mut task,
        "failureReason",
        row.get::<_, Option<String>>("failure_reason")?,
    );
    insert_optional(
        &mut task,
        "filesChanged",
        row.get::<_, Option<i64>>("files_changed")?,
    );
    insert_optional(
        &mut task,
        "settledAt",
        row.get::<_, Option<i64>>("settled_at")?,
    );
    insert_optional(
        &mut task,
        "historyDeletedAt",
        row.get::<_, Option<i64>>("history_deleted_at")?,
    );
    insert_optional(
        &mut task,
        "registeredAt",
        row.get::<_, Option<i64>>("registered_at")?,
    );
    let schedulable: i64 = row.get("schedulable")?;
    let is_terminal = TERMINAL.contains(&status.as_str());
    task.insert(
        "schedulable".to_owned(),
        Value::Bool(schedulable == 1 && has_selection && !is_terminal),
    );
    insert_optional(
        &mut task,
        "queuePosition",
        row.get::<_, Option<i64>>("queue_position")?,
    );
    insert_optional(
        &mut task,
        "nextPollAt",
        row.get::<_, Option<i64>>("next_poll_at")?,
    );
    task.insert(
        "createdAt".to_owned(),
        Value::from(row.get::<_, i64>("created_at")?),
    );
    task.insert(
        "updatedAt".to_owned(),
        Value::from(row.get::<_, i64>("updated_at")?),
    );
    Ok(Value::Object(task))
}

fn insert_optional<T: Into<Value>>(object: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), value.into());
    }
}

pub fn tasks_index_path(data_base_dir: &Path) -> std::path::PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite")
}
