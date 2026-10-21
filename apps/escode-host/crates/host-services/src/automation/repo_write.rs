//! automation 写面与调度状态机（TS `automationRepo.ts` 的 Rust 子集）。
//!
//! 仓库只做存储与原子状态迁移；cron 表达式解析 / next_run_at 计算由 service 算好后传入。

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use uuid::Uuid;

use super::repo::{AutomationRepo, row_to_automation, row_to_run};

pub const AUTOMATION_CREATE_LIMIT: i64 = 20;
pub const CLAIM_STALE_MS: i64 = 10 * 60_000;
pub const DISPATCH_MAX_ATTEMPTS: i64 = 5;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn workspace_key(params: &Value) -> Result<String, String> {
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

fn text(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn serialized(params: &Value, key: &str) -> Option<String> {
    match params.get(key) {
        None => None,
        Some(value) if value.is_null() => Some("null".to_owned()),
        Some(value) => Some(serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())),
    }
}

struct Row {
    automation_id: String,
    title: String,
    cron_expr: String,
    prompt: String,
    model: Option<String>,
    provider: Option<String>,
    model_selection: Option<String>,
    workspace_key: String,
    workspace_path: String,
    workspace_identity: Option<String>,
    target_task_id: Option<String>,
    location_kind: String,
    recurring: i64,
    max_runs: Option<i64>,
    end_at: Option<i64>,
    schedule_rule: Option<String>,
    schedule_edited_by_user: i64,
    run_count: i64,
    scheduled_run_count: i64,
    enabled: i64,
    lifecycle_status: String,
    next_run_at: Option<i64>,
    last_run_at: Option<i64>,
    running: i64,
    claimed_at: Option<i64>,
    dispatch_status: String,
    dispatch_attempts: i64,
    retry_at: Option<i64>,
    last_error: Option<String>,
    mode: Option<String>,
    thought_level: Option<String>,
    created_at: i64,
    updated_at: i64,
}

fn read_row(conn: &Connection, id: &str, scope: Option<&str>) -> Result<Option<Row>, String> {
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

fn write_row(conn: &Connection, row: &Row) -> Result<(), String> {
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

fn value(conn: &Connection, id: &str) -> Result<Value, String> {
    conn.query_row(
        "SELECT * FROM automations WHERE automation_id = ?1",
        [id],
        row_to_automation,
    )
    .map_err(|error| error.to_string())
}

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

    /// 编辑定义字段；`next_run_at`/`lifecycle_status` 由调用方按 cron 语义算好传入。
    pub fn update(
        &self,
        id: &str,
        params: &Value,
        next_run_at: Option<Option<i64>>,
        lifecycle_status: Option<&str>,
        reset_retry: bool,
        scope: Option<&str>,
    ) -> Result<Option<Value>, String> {
        let conn = self.open()?;
        let Some(existing) = read_row(&conn, id, scope)? else {
            return Ok(None);
        };
        let now = now_ms();
        let mut next = existing;
        if let Some(value) = text(params, "title") {
            next.title = value;
        }
        if let Some(value) = text(params, "cronExpr") {
            next.cron_expr = value;
        }
        if let Some(value) = text(params, "prompt") {
            next.prompt = value;
        }
        if let Some(value) = serialized(params, "modelSelection") {
            next.model_selection = Some(value);
        }
        if let Some(value) = params.get("mode") {
            next.mode = if value.is_null() {
                None
            } else {
                value.as_str().map(str::to_owned)
            };
        }
        if let Some(value) = params["recurring"].as_bool() {
            next.recurring = value as i64;
        }
        if let Some(value) = params.get("maxRuns") {
            next.max_runs = if value.is_null() {
                None
            } else {
                value.as_i64()
            };
        }
        if let Some(value) = params.get("endAt") {
            next.end_at = if value.is_null() {
                None
            } else {
                value.as_i64()
            };
        }
        if let Some(value) = params.get("scheduleRule") {
            next.schedule_rule = if value.is_null() {
                None
            } else {
                Some(serde_json::to_string(value).unwrap_or_default())
            };
        }
        if let Some(value) = params["scheduleEditedByUser"].as_bool() {
            next.schedule_edited_by_user = value as i64;
        }
        if let Some(value) = next_run_at {
            next.next_run_at = value;
        }
        if let Some(status) = lifecycle_status {
            next.lifecycle_status = status.to_owned();
            next.enabled = (status == "active") as i64;
        }
        if reset_retry {
            next.dispatch_attempts = 0;
            next.retry_at = None;
            next.dispatch_status = "idle".to_owned();
        }
        next.updated_at = now;
        write_row(&conn, &next)?;
        value(&conn, id).map(Some)
    }

    /// 终态任务手动重跑：回 active、清计数与重试态。
    pub fn restart(
        &self,
        id: &str,
        next_run_at: Option<i64>,
        scope: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE automations SET
              lifecycle_status='active', enabled=1, run_count=0, scheduled_run_count=0,
              dispatch_attempts=0, retry_at=NULL, dispatch_status='idle', running=0,
              claimed_at=NULL, next_run_at=?2, last_error=NULL, updated_at=?3
             WHERE automation_id=?1 AND (?4 IS NULL OR workspace_key=?4)",
            params![id, next_run_at, now_ms(), scope],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
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

    /// 认领到期项（single-flight）。
    pub fn claim_due(&self, now: i64) -> Result<Vec<Value>, String> {
        let conn = self.open()?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| error.to_string())?;
        let result = (|| {
            conn.execute(
                "UPDATE automations SET lifecycle_status='completed', enabled=0, next_run_at=NULL,
                  retry_at=NULL, running=0, claimed_at=NULL, updated_at=?1
                 WHERE enabled=1 AND end_at IS NOT NULL AND end_at < ?1",
                [now],
            )
            .map_err(|error| error.to_string())?;
            conn.execute(
                "UPDATE automations SET running=0, claimed_at=NULL \
                 WHERE running=1 AND claimed_at IS NOT NULL AND claimed_at <= ?1",
                [now - CLAIM_STALE_MS],
            )
            .map_err(|error| error.to_string())?;
            let mut statement = conn
                .prepare(
                    "SELECT * FROM automations \
                     WHERE enabled=1 AND running=0 AND \
                     ((retry_at IS NOT NULL AND retry_at <= ?1) \
                      OR (retry_at IS NULL AND next_run_at IS NOT NULL AND next_run_at <= ?1))",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([now], row_to_automation)
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            drop(statement);
            let mut claimed = Vec::new();
            for row in rows {
                let id = row["automationId"].as_str().unwrap_or_default();
                let changed = conn
                    .execute(
                        "UPDATE automations SET running=1, claimed_at=?2, \
                          dispatch_status='claimed', updated_at=?2 \
                         WHERE automation_id=?1 AND running=0",
                        params![id, now],
                    )
                    .map_err(|error| error.to_string())?;
                if changed == 1 {
                    claimed.push(value(&conn, id)?);
                }
            }
            Ok(claimed)
        })();
        match result {
            Ok(rows) => {
                conn.execute_batch("COMMIT")
                    .map_err(|error| error.to_string())?;
                Ok(rows)
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// 派发成功结算（展示/定时计数、清重试、按上限转终态）。
    pub fn mark_dispatched(
        &self,
        id: &str,
        dispatched_at: i64,
        next_run_at: Option<i64>,
    ) -> Result<(), String> {
        let conn = self.open()?;
        let Some(row) = read_row(&conn, id, None)? else {
            return Ok(());
        };
        let run_count = row.run_count + 1;
        let scheduled = row.scheduled_run_count + 1;
        let reached_max = row.recurring == 0 && scheduled >= row.max_runs.unwrap_or(1);
        let reached_end = row
            .end_at
            .is_some_and(|end| next_run_at.is_none_or(|next| next > end));
        let done = reached_max || reached_end;
        conn.execute(
            "UPDATE automations SET
              run_count=?2, scheduled_run_count=?3, last_run_at=?4,
              dispatch_status='dispatched', dispatch_attempts=0, retry_at=NULL, last_error=NULL,
              running=0, claimed_at=NULL, lifecycle_status=?5, enabled=?6, next_run_at=?7,
              updated_at=?4
             WHERE automation_id=?1",
            params![
                id,
                run_count,
                scheduled,
                dispatched_at,
                if done { "completed" } else { "active" },
                (!done) as i64,
                if done { None } else { next_run_at },
            ],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 派发失败：transient 退避；达上限后循环任务跳过本轮，有限次任务转 failed。
    pub fn mark_dispatch_failed(
        &self,
        id: &str,
        failed_at: i64,
        error: &str,
        permanent: bool,
        next_run_at: Option<i64>,
    ) -> Result<(), String> {
        let conn = self.open()?;
        let Some(row) = read_row(&conn, id, None)? else {
            return Ok(());
        };
        if permanent {
            conn.execute(
                "UPDATE automations SET dispatch_status='failed_to_dispatch',
                  lifecycle_status='failed', enabled=0, running=0, claimed_at=NULL,
                  last_error=?2, updated_at=?3 WHERE automation_id=?1",
                params![id, error, failed_at],
            )
            .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let attempts = row.dispatch_attempts + 1;
        if attempts >= DISPATCH_MAX_ATTEMPTS {
            if row.recurring == 1 {
                conn.execute(
                    "UPDATE automations SET dispatch_status='idle', dispatch_attempts=0,
                      retry_at=NULL, running=0, claimed_at=NULL, next_run_at=?2,
                      last_error=?3, updated_at=?4 WHERE automation_id=?1",
                    params![id, next_run_at, error, failed_at],
                )
                .map_err(|error| error.to_string())?;
            } else {
                conn.execute(
                    "UPDATE automations SET dispatch_status='failed_to_dispatch',
                      lifecycle_status='failed', enabled=0, running=0, claimed_at=NULL,
                      last_error=?2, updated_at=?3 WHERE automation_id=?1",
                    params![id, error, failed_at],
                )
                .map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
        let backoff =
            (5_000_i64 * 2_i64.pow(u32::try_from(attempts - 1).unwrap_or(0))).min(5 * 60_000);
        conn.execute(
            "UPDATE automations SET dispatch_status='failed_to_dispatch',
              dispatch_attempts=?2, retry_at=?3, running=0, claimed_at=NULL,
              last_error=?4, updated_at=?5 WHERE automation_id=?1",
            params![id, attempts, failed_at + backoff, error, failed_at],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 释放认领（关机/退出，不记失败不推进）。
    pub fn release_claim(&self, id: &str) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE automations SET running=0, claimed_at=NULL, dispatch_status='idle', \
              updated_at=?2 WHERE automation_id=?1 AND running=1",
            params![id, now_ms()],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// manual run 派发成功：run 台账置 dispatched、automation 累计次数 +1、释放 single-flight。
    pub fn mark_manual_run_dispatched(
        &self,
        run_id: &str,
        session_id: Option<&str>,
        dispatched_at: i64,
    ) -> Result<bool, String> {
        let conn = self.open()?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| error.to_string())?;
        let result = (|| {
            let run = conn
                .query_row(
                    "SELECT automation_id, workspace_key, dispatch_status \
                     FROM automation_runs WHERE run_id=?1 AND trigger='manual'",
                    [run_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| error.to_string())?;
            let Some((automation_id, workspace_key, status)) = run else {
                return Ok(false);
            };
            if status == "dispatched" {
                return Ok(false);
            }
            conn.execute(
                "UPDATE automation_runs SET dispatch_status='dispatched', \
                  session_id=COALESCE(?2, session_id), error=NULL, updated_at=?3 \
                 WHERE run_id=?1 AND trigger='manual' AND dispatch_status<>'dispatched'",
                params![run_id, session_id, dispatched_at],
            )
            .map_err(|error| error.to_string())?;
            let changed = conn
                .execute(
                    "UPDATE automations SET run_count=run_count+1, last_run_at=?3, \
                      updated_at=?3 WHERE automation_id=?1 AND workspace_key=?2",
                    params![automation_id, workspace_key, dispatched_at],
                )
                .map_err(|error| error.to_string())?;
            Ok(changed > 0)
        })();
        match result {
            Ok(changed) => {
                conn.execute_batch("COMMIT")
                    .map_err(|error| error.to_string())?;
                Ok(changed)
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// manual run 派发失败：run 记 failed、释放 single-flight（不动 cron 计划）。
    pub fn fail_manual_run(&self, run_id: &str, error: &str) -> Result<(), String> {
        let conn = self.open()?;
        let now = now_ms();
        conn.execute(
            "UPDATE automation_runs SET dispatch_status='failed_to_dispatch', \
              outcome='failed', error=?2, updated_at=?3 WHERE run_id=?1",
            params![run_id, error, now],
        )
        .map_err(|error| error.to_string())?;
        conn.execute(
            "UPDATE automations SET running=0, claimed_at=NULL, updated_at=?2 \
             WHERE automation_id=(SELECT automation_id FROM automation_runs WHERE run_id=?1)",
            params![run_id, now],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// session runtime 回写运行结果。
    pub fn mark_run_outcome(
        &self,
        run_id: &str,
        outcome: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE automation_runs SET outcome=?2, error=?3, updated_at=?4 WHERE run_id=?1",
            params![run_id, outcome, error, now_ms()],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 定时 run 台账：同一 runId 幂等（重试复用 scheduledAt，runId 稳定）。
    pub fn upsert_run_claimed(
        &self,
        run_id: &str,
        automation_id: &str,
        workspace_key: &str,
        scheduled_at: i64,
    ) -> Result<(), String> {
        let conn = self.open()?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO automation_runs (
               run_id, automation_id, workspace_key, scheduled_at, trigger,
               model_selection, dispatch_status, attempts, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'schedule', NULL, 'claimed', 0, ?5, ?5)
             ON CONFLICT(run_id) DO UPDATE SET
               dispatch_status='claimed', error=NULL, updated_at=?5",
            params![run_id, automation_id, workspace_key, scheduled_at, now],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn mark_run_dispatch(
        &self,
        run_id: &str,
        status: &str,
        session_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE automation_runs SET dispatch_status=?2, \
              session_id=COALESCE(?3, session_id), error=?4, updated_at=?5 WHERE run_id=?1",
            params![run_id, status, session_id, error, now_ms()],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 错过的定时窗口：跳过不补跑，只把 next_run_at 前推并释放认领。
    pub fn skip_due(&self, id: &str, next_run_at: Option<i64>) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE automations SET next_run_at=?2, running=0, claimed_at=NULL, \
              dispatch_status='idle', retry_at=NULL, updated_at=?3 \
             WHERE automation_id=?1",
            params![id, next_run_at, now_ms()],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
