//! `automations` / `automation_runs` 本地读取与生命周期（TS `automationRepo.ts` 子集）。

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

const MODES: [&str; 10] = [
    "default",
    "yolo",
    "guarded",
    "plan",
    "edit",
    "acceptEdits",
    "auto",
    "dontAsk",
    "bypassPermissions",
    "build",
];

#[derive(Clone)]
pub struct AutomationRepo {
    path: PathBuf,
}

impl AutomationRepo {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn open(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.path).map_err(|error| error.to_string())?;
        connection
            .busy_timeout(Duration::from_millis(5000))
            .map_err(|error| error.to_string())?;
        Ok(connection)
    }

    pub fn list(&self, workspace_key: Option<&str>) -> Result<Vec<Value>, String> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(
                "SELECT * FROM automations \
                 WHERE (?1 IS NULL OR workspace_key = ?1) ORDER BY created_at DESC",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map([workspace_key], row_to_automation)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn delete(&self, id: &str, workspace_key: Option<&str>) -> Result<bool, String> {
        let connection = self.open()?;
        let changed = connection
            .execute(
                "DELETE FROM automations WHERE automation_id = ?1 \
                 AND (?2 IS NULL OR workspace_key = ?2)",
                rusqlite::params![id, workspace_key],
            )
            .map_err(|error| error.to_string())?;
        Ok(changed > 0)
    }

    pub fn get(&self, id: &str, workspace_key: Option<&str>) -> Result<Option<Value>, String> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT * FROM automations WHERE automation_id = ?1 \
                 AND (?2 IS NULL OR workspace_key = ?2)",
                rusqlite::params![id, workspace_key],
                row_to_automation,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        workspace_key: Option<&str>,
        now: i64,
    ) -> Result<(), String> {
        let connection = self.open()?;
        connection
            .execute(
                "UPDATE automations SET enabled = ?2, lifecycle_status = ?3, updated_at = ?4 \
                 WHERE automation_id = ?1 AND (?5 IS NULL OR workspace_key = ?5)",
                rusqlite::params![
                    id,
                    if enabled { 1 } else { 0 },
                    if enabled { "active" } else { "paused" },
                    now,
                    workspace_key
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn list_runs(
        &self,
        automation_id: &str,
        workspace_key: Option<&str>,
    ) -> Result<Vec<Value>, String> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(
                "SELECT * FROM automation_runs WHERE automation_id = ?1 \
                 AND (?2 IS NULL OR workspace_key = ?2) ORDER BY created_at DESC",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map(rusqlite::params![automation_id, workspace_key], row_to_run)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn delete_run(&self, run_id: &str, workspace_key: Option<&str>) -> Result<(), String> {
        let connection = self.open()?;
        connection
            .execute(
                "DELETE FROM automation_runs WHERE run_id = ?1 \
                 AND (?2 IS NULL OR workspace_key = ?2)",
                rusqlite::params![run_id, workspace_key],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

pub(super) fn row_to_automation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let mut task = Map::new();
    task.insert(
        "automationId".to_owned(),
        Value::String(row.get("automation_id")?),
    );
    task.insert("title".to_owned(), Value::String(row.get("title")?));
    task.insert("cronExpr".to_owned(), Value::String(row.get("cron_expr")?));
    task.insert("prompt".to_owned(), Value::String(row.get("prompt")?));
    if let Some(selection) = selection(row.get::<_, Option<String>>("model_selection")?) {
        task.insert("modelSelection".to_owned(), selection);
    }
    if let Some(mode) = row
        .get::<_, Option<String>>("mode")?
        .filter(|mode| MODES.contains(&mode.as_str()))
    {
        task.insert("mode".to_owned(), Value::String(mode));
    }
    task.insert(
        "workspaceKey".to_owned(),
        Value::String(row.get("workspace_key")?),
    );
    task.insert(
        "workspacePath".to_owned(),
        Value::String(row.get("workspace_path")?),
    );
    optional_string(
        &mut task,
        "workspaceIdentity",
        row.get("workspace_identity")?,
    );
    optional_string(&mut task, "targetTaskId", row.get("target_task_id")?);
    task.insert(
        "locationKind".to_owned(),
        Value::String(
            if row.get::<_, Option<String>>("location_kind")?.as_deref() == Some("remote") {
                "remote"
            } else {
                "local"
            }
            .to_owned(),
        ),
    );
    task.insert(
        "recurring".to_owned(),
        Value::Bool(row.get::<_, i64>("recurring")? == 1),
    );
    optional_i64(&mut task, "maxRuns", row.get("max_runs")?);
    optional_i64(&mut task, "endAt", row.get("end_at")?);
    if let Some(raw) = row.get::<_, Option<String>>("schedule_rule")?
        && let Ok(rule) = serde_json::from_str::<Value>(&raw)
    {
        task.insert("scheduleRule".to_owned(), rule);
    }
    if row.get::<_, i64>("schedule_edited_by_user")? == 1 {
        task.insert("scheduleEditedByUser".to_owned(), Value::Bool(true));
    }
    task.insert(
        "runCount".to_owned(),
        Value::from(row.get::<_, i64>("run_count")?),
    );
    task.insert(
        "enabled".to_owned(),
        Value::Bool(row.get::<_, i64>("enabled")? == 1),
    );
    task.insert(
        "lifecycleStatus".to_owned(),
        Value::String(row.get("lifecycle_status")?),
    );
    optional_i64(&mut task, "nextRunAt", row.get("next_run_at")?);
    optional_i64(&mut task, "lastRunAt", row.get("last_run_at")?);
    task.insert(
        "dispatchStatus".to_owned(),
        Value::String(row.get("dispatch_status")?),
    );
    task.insert(
        "dispatchAttempts".to_owned(),
        Value::from(row.get::<_, i64>("dispatch_attempts")?),
    );
    optional_i64(&mut task, "retryAt", row.get("retry_at")?);
    optional_string(&mut task, "lastError", row.get("last_error")?);
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

pub(super) fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let mut run = Map::new();
    run.insert("runId".to_owned(), Value::String(row.get("run_id")?));
    run.insert(
        "automationId".to_owned(),
        Value::String(row.get("automation_id")?),
    );
    run.insert(
        "workspaceKey".to_owned(),
        Value::String(row.get("workspace_key")?),
    );
    optional_i64(&mut run, "scheduledAt", row.get("scheduled_at")?);
    run.insert("trigger".to_owned(), Value::String(row.get("trigger")?));
    if let Some(selection) = selection(row.get::<_, Option<String>>("model_selection")?) {
        run.insert("modelSelection".to_owned(), selection);
    }
    run.insert(
        "dispatchStatus".to_owned(),
        Value::String(row.get("dispatch_status")?),
    );
    optional_string(&mut run, "outcome", row.get("outcome")?);
    optional_string(&mut run, "sessionId", row.get("session_id")?);
    optional_string(&mut run, "error", row.get("error")?);
    run.insert(
        "attempts".to_owned(),
        Value::from(row.get::<_, i64>("attempts")?),
    );
    run.insert(
        "createdAt".to_owned(),
        Value::from(row.get::<_, i64>("created_at")?),
    );
    run.insert(
        "updatedAt".to_owned(),
        Value::from(row.get::<_, i64>("updated_at")?),
    );
    Ok(Value::Object(run))
}

fn selection(raw: Option<String>) -> Option<Value> {
    raw.and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
}

fn optional_string(object: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), Value::String(value));
    }
}

fn optional_i64(object: &mut Map<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), Value::from(value));
    }
}

pub fn tasks_index_path(data_base_dir: &std::path::Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite")
}

#[allow(dead_code)]
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
