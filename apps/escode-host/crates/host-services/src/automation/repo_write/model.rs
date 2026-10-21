//! `automations` 行映射与三类原子操作共用的读写工具。

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::super::repo::row_to_automation;

pub(super) const AUTOMATION_CREATE_LIMIT: i64 = 20;
pub(super) const CLAIM_STALE_MS: i64 = 10 * 60_000;
pub(super) const DISPATCH_MAX_ATTEMPTS: i64 = 5;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub(super) fn workspace_key(params: &Value) -> Result<String, String> {
    let path = params["workspacePath"]
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "workspacePath is required".to_owned())?;
    Ok(params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .unwrap_or(path)
        .to_owned())
}

pub(super) fn text(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn serialized(params: &Value, key: &str) -> Option<String> {
    match params.get(key) {
        None => None,
        Some(value) if value.is_null() => Some("null".to_owned()),
        Some(value) => Some(serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())),
    }
}

pub(super) struct Row {
    pub(super) automation_id: String,
    pub(super) title: String,
    pub(super) cron_expr: String,
    pub(super) prompt: String,
    pub(super) model: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) model_selection: Option<String>,
    pub(super) workspace_key: String,
    pub(super) workspace_path: String,
    pub(super) workspace_identity: Option<String>,
    pub(super) target_task_id: Option<String>,
    pub(super) location_kind: String,
    pub(super) recurring: i64,
    pub(super) max_runs: Option<i64>,
    pub(super) end_at: Option<i64>,
    pub(super) schedule_rule: Option<String>,
    pub(super) schedule_edited_by_user: i64,
    pub(super) run_count: i64,
    pub(super) scheduled_run_count: i64,
    pub(super) enabled: i64,
    pub(super) lifecycle_status: String,
    pub(super) next_run_at: Option<i64>,
    pub(super) last_run_at: Option<i64>,
    pub(super) running: i64,
    pub(super) claimed_at: Option<i64>,
    pub(super) dispatch_status: String,
    pub(super) dispatch_attempts: i64,
    pub(super) retry_at: Option<i64>,
    pub(super) last_error: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) thought_level: Option<String>,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
}

pub(super) fn read_row(
    conn: &Connection,
    id: &str,
    scope: Option<&str>,
) -> Result<Option<Row>, String> {
    conn.query_row(
        "SELECT * FROM automations WHERE automation_id = ?1 \
         AND (?2 IS NULL OR workspace_key = ?2)",
        params![id, scope],
        |row| {
            Ok(Row {
                automation_id: row.get("automation_id")?,
                title: row.get("title")?,
                cron_expr: row.get("cron_expr")?,
                prompt: row.get("prompt")?,
                model: row.get("model")?,
                provider: row.get("provider")?,
                model_selection: row.get("model_selection")?,
                workspace_key: row.get("workspace_key")?,
                workspace_path: row.get("workspace_path")?,
                workspace_identity: row.get("workspace_identity")?,
                target_task_id: row.get("target_task_id")?,
                location_kind: row.get("location_kind")?,
                recurring: row.get("recurring")?,
                max_runs: row.get("max_runs")?,
                end_at: row.get("end_at")?,
                schedule_rule: row.get("schedule_rule")?,
                schedule_edited_by_user: row.get("schedule_edited_by_user")?,
                run_count: row.get("run_count")?,
                scheduled_run_count: row.get("scheduled_run_count")?,
                enabled: row.get("enabled")?,
                lifecycle_status: row.get("lifecycle_status")?,
                next_run_at: row.get("next_run_at")?,
                last_run_at: row.get("last_run_at")?,
                running: row.get("running")?,
                claimed_at: row.get("claimed_at")?,
                dispatch_status: row.get("dispatch_status")?,
                dispatch_attempts: row.get("dispatch_attempts")?,
                retry_at: row.get("retry_at")?,
                last_error: row.get("last_error")?,
                mode: row.get("mode")?,
                thought_level: row.get("thought_level")?,
                created_at: row.get("created_at")?,
                updated_at: row.get("updated_at")?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub(super) fn write_row(conn: &Connection, row: &Row) -> Result<(), String> {
    conn.execute(
        "UPDATE automations SET
          title=?2, cron_expr=?3, prompt=?4, model=?5, provider=?6, model_selection=?7,
          workspace_key=?8, workspace_path=?9, workspace_identity=?10, target_task_id=?11,
          location_kind=?12, recurring=?13, max_runs=?14, end_at=?15, schedule_rule=?16,
          schedule_edited_by_user=?17, run_count=?18, scheduled_run_count=?19, enabled=?20,
          lifecycle_status=?21, next_run_at=?22, last_run_at=?23, running=?24, claimed_at=?25,
          dispatch_status=?26, dispatch_attempts=?27, retry_at=?28, last_error=?29,
          mode=?30, thought_level=?31, created_at=?32, updated_at=?33
         WHERE automation_id=?1",
        params![
            row.automation_id,
            row.title,
            row.cron_expr,
            row.prompt,
            row.model,
            row.provider,
            row.model_selection,
            row.workspace_key,
            row.workspace_path,
            row.workspace_identity,
            row.target_task_id,
            row.location_kind,
            row.recurring,
            row.max_runs,
            row.end_at,
            row.schedule_rule,
            row.schedule_edited_by_user,
            row.run_count,
            row.scheduled_run_count,
            row.enabled,
            row.lifecycle_status,
            row.next_run_at,
            row.last_run_at,
            row.running,
            row.claimed_at,
            row.dispatch_status,
            row.dispatch_attempts,
            row.retry_at,
            row.last_error,
            row.mode,
            row.thought_level,
            row.created_at,
            row.updated_at,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn value(conn: &Connection, id: &str) -> Result<Value, String> {
    conn.query_row(
        "SELECT * FROM automations WHERE automation_id = ?1",
        [id],
        row_to_automation,
    )
    .map_err(|error| error.to_string())
}
