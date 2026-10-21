//! automation 的 cron / scheduleRule 下一次触发时间（TS `automationCron.ts` 的 Rust 对拍实现）。
//!
//! TS 用 croner 只做「解析 + 算下次时间」，调度循环由 scheduler 自己轮询；这里保持同一分工。
//! 所有日历计算使用本机时区（`chrono::Local`），与 TS `new Date(...)` 一致。
//!
//! scheduleRule 的日历计算与规则构造拆到 `schedule_rule` / `build`，本模块只保留
//! cron 表达式入口与一次性任务的到点判定。

use std::str::FromStr;

use chrono::{DateTime, Local, TimeZone};
use cron::Schedule;
use serde_json::Value;

mod build;
mod schedule_rule;

pub use build::{
    build_interval_schedule_rule, build_relative_delay_schedule,
    infer_minute_interval_schedule_rule, schedule_rule_definition,
};
pub use schedule_rule::compute_schedule_rule_next_run_at;

const ONE_SHOT_MISSED_RUN_GRACE_MS: i64 = 60 * 1_000;
const ONE_SHOT_STALE_TARGET_WINDOW_MS: i64 = 30 * 60 * 1_000;

pub(super) fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// TS croner 接受 5 段；`cron` crate 的秒字段可选，这里统一补 `0` 秒。
fn parse_schedule(cron_expr: &str) -> Option<Schedule> {
    let trimmed = cron_expr.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if trimmed.split_whitespace().count() == 5 {
        format!("0 {trimmed}")
    } else {
        trimmed.to_owned()
    };
    Schedule::from_str(&normalized).ok()
}

/// 严格晚于 `from` 的下一次触发；无未来触发返回 `None`。
pub fn compute_next_run_at(cron_expr: &str, from: i64) -> Option<i64> {
    let schedule = parse_schedule(cron_expr)?;
    let from = local(from)?;
    schedule
        .after(&from)
        .next()
        .map(|next| next.timestamp_millis())
}

/// 固定日历 cron 的最近一次触发（只用于一次性任务的“刚错过”判定）。
fn previous_run(cron_expr: &str, from: i64) -> Option<i64> {
    let schedule = parse_schedule(cron_expr)?;
    let start = local(from - 2 * 24 * 60 * 60 * 1_000)?;
    let mut previous = None;
    for date in schedule.after(&start).take(10_000) {
        let ms = date.timestamp_millis();
        if ms >= from {
            break;
        }
        previous = Some(ms);
    }
    previous
}

pub fn compute_automation_next_run_at(
    cron_expr: &str,
    schedule_rule: Option<&Value>,
    from: i64,
) -> Option<i64> {
    schedule_rule
        .map(|rule| compute_schedule_rule_next_run_at(rule, from))
        .unwrap_or_else(|| compute_next_run_at(cron_expr, from))
}

fn fixed_calendar_cron(cron_expr: &str) -> bool {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    parts.len() == 5
        && parts[..4]
            .iter()
            .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
        && parts[4] == "*"
}

/// 新建任务的首次触发时间（TS `computeInitialAutomationNextRunAt`）。
pub fn compute_initial_next_run_at(
    cron_expr: &str,
    recurring: bool,
    schedule_rule: Option<&Value>,
    from: i64,
) -> Result<Option<i64>, String> {
    let next = compute_automation_next_run_at(cron_expr, schedule_rule, from);
    if recurring || schedule_rule.is_some() || !fixed_calendar_cron(cron_expr) {
        return Ok(next);
    }
    let Some(previous) = previous_run(cron_expr, from) else {
        return Ok(next);
    };
    let target_age = from - previous;
    if !(0..=ONE_SHOT_STALE_TARGET_WINDOW_MS).contains(&target_age) {
        return Ok(next);
    }
    let rolled = next.is_none_or(|next| next - from > ONE_SHOT_STALE_TARGET_WINDOW_MS);
    if !rolled {
        return Ok(next);
    }
    if target_age < ONE_SHOT_MISSED_RUN_GRACE_MS {
        return Ok(Some(from));
    }
    Err(format!(
        "一次性定时任务的目标时间（{}）已过去；相对时间请使用 delayMinutes，绝对时间请确认未来时刻后重试",
        local(previous)
            .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| previous.to_string())
    ))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn cron_next_run_uses_local_calendar() {
        let from = Local
            .with_ymd_and_hms(2026, 10, 21, 8, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let next = compute_next_run_at("0 9 * * *", from).unwrap();
        assert_eq!(
            local(next).unwrap().format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-21 09:00"
        );
    }

    #[test]
    fn schedule_rule_minute_is_anchored() {
        let anchor = Local
            .with_ymd_and_hms(2026, 10, 21, 8, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let rule = json!({"unit":"minute","interval":5,"hour":8,"minute":0,"anchorAt":anchor});
        assert_eq!(
            compute_schedule_rule_next_run_at(&rule, anchor + 1).unwrap(),
            anchor + 5 * 60 * 1_000
        );
    }

    #[test]
    fn one_shot_stale_target_is_rejected() {
        let from = Local
            .with_ymd_and_hms(2026, 10, 21, 9, 10, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let error = compute_initial_next_run_at("5 9 21 10 *", false, None, from).unwrap_err();
        assert!(error.contains("已过去"));
    }
}
