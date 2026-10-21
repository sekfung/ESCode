//! 新建 automation 与 manual run 的 single-flight 认领。

use rusqlite::params;
use serde_json::Value;
use uuid::Uuid;

use super::super::repo::{AutomationRepo, row_to_run};
use super::model::{
    AUTOMATION_CREATE_LIMIT, CLAIM_STALE_MS, Row, now_ms, read_row, serialized, text, value,
    workspace_key,
};

impl AutomationRepo {
    /// 创建（上限 20；`BEGIN IMMEDIATE` 串行化计数与插入）。
    pub fn create(
        &self,
        params: &Value,
        next_run_at: Option<i64>,
        lifecycle_status: &str,
    ) -> Result<Value, String> {
        let conn = self.open()?;
        let now = now_ms();
        let id = format!("automation-{}", Uuid::new_v4());
        let key = workspace_key(params)?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| error.to_string())?;
        let result = (|| {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM automations", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if count >= AUTOMATION_CREATE_LIMIT {
                return Err(format!(
                    "[AUTOMATION_CREATE_LIMIT_REACHED] At most {AUTOMATION_CREATE_LIMIT} automations may be retained. Delete an existing automation before creating another."
                ));
            }
            let row = Row {
                automation_id: id.clone(),
                title: text(params, "title").unwrap_or_default(),
                cron_expr: text(params, "cronExpr").unwrap_or_default(),
                prompt: text(params, "prompt").unwrap_or_default(),
                model: None,
                provider: None,
                model_selection: serialized(params, "modelSelection"),
                workspace_key: key,
                workspace_path: text(params, "workspacePath").unwrap_or_default(),
                workspace_identity: text(params, "workspaceIdentity"),
                target_task_id: text(params, "targetTaskId"),
                location_kind: "local".to_owned(),
                recurring: params["recurring"].as_bool().unwrap_or(true) as i64,
                max_runs: params["maxRuns"].as_i64(),
                end_at: params["endAt"].as_i64(),
                schedule_rule: params
                    .get("scheduleRule")
                    .filter(|value| !value.is_null())
                    .map(|value| serde_json::to_string(value).unwrap_or_default()),
                schedule_edited_by_user: 0,
                run_count: 0,
                scheduled_run_count: 0,
                enabled: (lifecycle_status != "completed") as i64,
                lifecycle_status: lifecycle_status.to_owned(),
                next_run_at,
                last_run_at: None,
                running: 0,
                claimed_at: None,
                dispatch_status: "idle".to_owned(),
                dispatch_attempts: 0,
                retry_at: None,
                last_error: None,
                mode: text(params, "mode"),
                thought_level: None,
                created_at: now,
                updated_at: now,
            };
            conn.execute(
                "INSERT INTO automations (
                  automation_id, title, cron_expr, prompt, model, provider, model_selection,
                  workspace_key, workspace_path, workspace_identity, target_task_id, location_kind,
                  recurring, max_runs, end_at, schedule_rule, schedule_edited_by_user,
                  run_count, scheduled_run_count, enabled, lifecycle_status, next_run_at,
                  last_run_at, running, claimed_at, dispatch_status, dispatch_attempts,
                  retry_at, last_error, mode, thought_level, created_at, updated_at
                ) VALUES (
                  ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                  ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31,
                  ?32, ?33
                )",
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
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")
                    .map_err(|error| error.to_string())?;
                value(&conn, &id)
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// 立即运行：写 manual run 并占用 single-flight 锁；重复运行返回 `None`。
    pub fn run_now(
        &self,
        id: &str,
        scope: Option<&str>,
        now: i64,
    ) -> Result<Option<(Value, Value)>, String> {
        let conn = self.open()?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| error.to_string())?;
        let result = (|| {
            conn.execute(
                "UPDATE automations SET running=0, claimed_at=NULL \
                 WHERE running=1 AND claimed_at IS NOT NULL AND claimed_at <= ?1",
                [now - CLAIM_STALE_MS],
            )
            .map_err(|error| error.to_string())?;
            let Some(row) = read_row(&conn, id, scope)? else {
                return Ok(None);
            };
            let changed = conn
                .execute(
                    "UPDATE automations SET running=1, claimed_at=?2, updated_at=?2 \
                     WHERE automation_id=?1 AND running=0 \
                     AND (?3 IS NULL OR workspace_key=?3)",
                    params![id, now, scope],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Ok(None);
            }
            let run_id = format!("{id}:manual:{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO automation_runs (
                  run_id, automation_id, workspace_key, scheduled_at, trigger,
                  model_selection, dispatch_status, attempts, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, 'manual', NULL, 'claimed', 1, ?4, ?4)",
                params![run_id, id, row.workspace_key, now],
            )
            .map_err(|error| error.to_string())?;
            let automation = value(&conn, id)?;
            let run = conn
                .query_row(
                    "SELECT * FROM automation_runs WHERE run_id=?1",
                    [&run_id],
                    row_to_run,
                )
                .map_err(|error| error.to_string())?;
            Ok(Some((automation, run)))
        })();
        match result {
            Ok(value) => {
                conn.execute_batch("COMMIT")
                    .map_err(|error| error.to_string())?;
                Ok(value)
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}
