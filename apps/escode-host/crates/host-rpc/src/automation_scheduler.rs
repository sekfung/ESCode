//! 常驻 automation scheduler（H5.3 的无窗口调度）。
//!
//! Host 进程内唯一的调度循环：每 20 s 认领到期 automation，跳过错过窗口，派发 createTask +
//! sendPrompt，并按成功/失败结算。Main 不持有调度状态。

use std::sync::Arc;
use std::time::Duration;

use escode_host_control::logging;
use escode_host_services::automation::AutomationService;
use escode_host_services::automation::cron::compute_automation_next_run_at;
use serde_json::{Value, json};

use crate::automation_dispatch::{dispatch_run, track_outcome};
use crate::domains_task_state::StateContext;

const POLL_INTERVAL: Duration = Duration::from_secs(20);
/// 比一次正常轮询明显大，避免把正常到点误判成关机/休眠期间的 misfire。
const MISFIRE_GRACE_MS: i64 = 5 * 60_000;

pub(crate) fn spawn(service: Arc<AutomationService>, state: StateContext) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            if let Err(error) = tick(&service, &state).await {
                logging::warn("automation", format!("scheduler tick failed: {error}"));
            }
        }
    });
}

async fn tick(service: &Arc<AutomationService>, state: &StateContext) -> Result<(), String> {
    let now = now_ms();
    let claimed = service.claim_due(now).await?;
    let claimed = claimed.as_array().cloned().unwrap_or_default();
    for automation in claimed {
        if let Err(error) = handle_claimed(service, state, &automation, now).await {
            logging::warn(
                "automation",
                format!(
                    "dispatch failed automation={}: {error}",
                    automation["automationId"].as_str().unwrap_or_default()
                ),
            );
        }
    }
    Ok(())
}

async fn handle_claimed(
    service: &Arc<AutomationService>,
    state: &StateContext,
    automation: &Value,
    now: i64,
) -> Result<(), String> {
    let automation_id = automation["automationId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let workspace_key = automation["workspaceKey"]
        .as_str()
        .or_else(|| automation["workspacePath"].as_str())
        .unwrap_or_default()
        .to_owned();
    let next_run_at = automation["nextRunAt"].as_i64();
    let retry_at = automation["retryAt"].as_i64();
    let scheduled_at = next_run_at.or(retry_at).unwrap_or(now);
    let is_retry = automation["dispatchAttempts"].as_i64().unwrap_or(0) > 0;

    if !is_retry && next_run_at.is_some_and(|next| next <= now - MISFIRE_GRACE_MS) {
        let next = if one_shot(automation) {
            None
        } else {
            next_run(automation, now)
        };
        logging::info(
            "automation",
            format!("skipped missed window automation={automation_id} scheduledAt={scheduled_at}"),
        );
        service.skip_due(automation_id, next).await?;
        return Ok(());
    }

    let run_id = format!("{automation_id}:{scheduled_at}");
    service
        .upsert_run_claimed(
            run_id.clone(),
            automation_id.clone(),
            workspace_key,
            scheduled_at,
        )
        .await?;
    let run = json!({"runId": run_id, "trigger": "schedule"});
    match dispatch_run(state, automation, &run).await {
        Ok(dispatched) => {
            let _ = service
                .mark_run_dispatch(
                    run_id.clone(),
                    "dispatched".to_owned(),
                    Some(dispatched.task_id),
                    None,
                )
                .await;
            track_outcome(Arc::clone(service), run_id, dispatched.terminal);
            service
                .mark_dispatched(automation_id, now, next_run(automation, now))
                .await
        }
        Err(error) => {
            let _ = service
                .mark_run_dispatch(
                    run_id,
                    "failed_to_dispatch".to_owned(),
                    None,
                    Some(error.message.clone()),
                )
                .await;
            service
                .mark_dispatch_failed(
                    automation_id,
                    now,
                    error.message,
                    false,
                    next_run(automation, now),
                )
                .await
        }
    }
}

fn one_shot(automation: &Value) -> bool {
    automation["recurring"].as_bool() == Some(false)
        && automation["maxRuns"].as_i64().unwrap_or(1) <= 1
}

fn next_run(automation: &Value, now: i64) -> Option<i64> {
    compute_automation_next_run_at(
        automation["cronExpr"].as_str().unwrap_or_default(),
        automation
            .get("scheduleRule")
            .filter(|value| !value.is_null()),
        now,
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
