//! automation 定义编辑、终态重跑、认领释放与错过窗口跳过。

use rusqlite::params;
use serde_json::Value;

use super::super::repo::AutomationRepo;
use super::model::{now_ms, read_row, serialized, text, value, write_row};

impl AutomationRepo {
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
