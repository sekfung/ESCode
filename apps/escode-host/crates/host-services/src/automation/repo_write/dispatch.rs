//! 调度状态机：到期认领、派发结算、manual run 台账回写。

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use super::super::repo::{AutomationRepo, row_to_automation};
use super::model::{CLAIM_STALE_MS, DISPATCH_MAX_ATTEMPTS, now_ms, read_row, value};

impl AutomationRepo {
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
}
