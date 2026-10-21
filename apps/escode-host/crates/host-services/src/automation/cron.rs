//! automation 的 cron / scheduleRule 下一次触发时间（TS `automationCron.ts` 的 Rust 对拍实现）。
//!
//! TS 用 croner 只做「解析 + 算下次时间」，调度循环由 scheduler 自己轮询；这里保持同一分工。
//! 所有日历计算使用本机时区（`chrono::Local`），与 TS `new Date(...)` 一致。

use std::str::FromStr;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use cron::Schedule;
use serde_json::{Map, Value, json};

const ONE_SHOT_MISSED_RUN_GRACE_MS: i64 = 60 * 1_000;
const ONE_SHOT_STALE_TARGET_WINDOW_MS: i64 = 30 * 60 * 1_000;

fn local(ms: i64) -> Option<DateTime<Local>> {
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

fn at_time(date: DateTime<Local>, hour: i64, minute: i64) -> Option<i64> {
    let hour = u32::try_from(hour).ok()?;
    let minute = u32::try_from(minute).ok()?;
    Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
        .earliest()
        .map(|value| value.timestamp_millis())
}

fn first_weekday_of_month(year: i32, month: u32, weekday: i64) -> Option<DateTime<Local>> {
    let first = Local.with_ymd_and_hms(year, month, 1, 0, 0, 0).earliest()?;
    let weekday = u32::try_from(weekday).ok()?;
    let offset = (weekday + 7 - first.weekday().num_days_from_sunday()) % 7;
    Local
        .with_ymd_and_hms(year, month, 1 + offset, 0, 0, 0)
        .earliest()
}

/// 自定义重复规则的下一次运行（TS `computeScheduleRuleNextRunAt`）。
pub fn compute_schedule_rule_next_run_at(rule: &Value, from: i64) -> Option<i64> {
    let unit = rule["unit"].as_str()?;
    let interval = rule["interval"].as_i64().unwrap_or(1).max(1);
    let anchor_at = rule["anchorAt"].as_i64()?;
    let hour = rule["hour"].as_i64().unwrap_or(0);
    let minute = rule["minute"].as_i64().unwrap_or(0);
    let list = |key: &str| -> Vec<i64> {
        rule[key]
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    };
    let anchor = local(anchor_at)?;

    match unit {
        "minute" => {
            let step = interval * 60 * 1_000;
            let steps = ((from - anchor_at) / step + 1).max(1);
            Some(anchor_at + steps * step)
        }
        "hourly" => {
            let base = Local
                .with_ymd_and_hms(
                    anchor.year(),
                    anchor.month(),
                    anchor.day(),
                    anchor.hour(),
                    u32::try_from(minute).ok()?,
                    0,
                )
                .earliest()?
                .timestamp_millis();
            let step = interval * 60 * 60 * 1_000;
            let steps = ((from - base) / step + 1).max(0);
            Some(base + steps * step)
        }
        "daily" => {
            for index in 0..36_600 {
                let date = anchor.checked_add_signed(chrono::Duration::days(index * interval))?;
                let candidate = at_time(date, hour, minute)?;
                if candidate > from {
                    return Some(candidate);
                }
            }
            None
        }
        "weekly" => {
            let mut weekdays = list("weekdays");
            if weekdays.is_empty() {
                weekdays.push(1);
            }
            weekdays.sort_unstable();
            let anchor_week = anchor
                .with_hour(0)?
                .with_minute(0)?
                .with_second(0)?
                .with_nanosecond(0)?;
            let monday = anchor_week.checked_sub_signed(chrono::Duration::days(i64::from(
                anchor_week.weekday().num_days_from_monday(),
            )))?;
            for week in (0..5_220).step_by(usize::try_from(interval).ok()?.max(1)) {
                for weekday in &weekdays {
                    let day_offset = (weekday + 6) % 7;
                    let date = monday.checked_add_signed(chrono::Duration::days(
                        (week as i64) * 7 + day_offset,
                    ))?;
                    let candidate = at_time(date, hour, minute)?;
                    if candidate > from {
                        return Some(candidate);
                    }
                }
            }
            None
        }
        "monthly" => {
            let monthly_mode = rule["monthlyMode"].as_str().unwrap_or("date");
            let month_days = list("monthDays");
            for offset in (0..=1_200).step_by(usize::try_from(interval).ok()?.max(1)) {
                let month =
                    anchor.checked_add_months(chrono::Months::new(u32::try_from(offset).ok()?))?;
                let month = Local
                    .with_ymd_and_hms(month.year(), month.month(), 1, 0, 0, 0)
                    .earliest()?;
                let candidates: Vec<DateTime<Local>> = if monthly_mode == "weekday" {
                    first_weekday_of_month(
                        month.year(),
                        month.month(),
                        list("weekdays").first().copied().unwrap_or(1),
                    )
                    .into_iter()
                    .collect()
                } else {
                    let mut days = if month_days.is_empty() {
                        vec![1]
                    } else {
                        month_days.clone()
                    };
                    days.sort_unstable();
                    days.into_iter()
                        .filter_map(|day| {
                            Local
                                .with_ymd_and_hms(
                                    month.year(),
                                    month.month(),
                                    u32::try_from(day).ok()?,
                                    0,
                                    0,
                                    0,
                                )
                                .earliest()
                        })
                        .filter(|date| date.month() == month.month())
                        .collect()
                };
                for date in candidates {
                    let candidate = at_time(date, hour, minute)?;
                    if candidate > from {
                        return Some(candidate);
                    }
                }
            }
            None
        }
        "yearly" => {
            let months = list("months");
            let month_days = list("monthDays");
            let target_month = months
                .first()
                .map(|month| (((month - 1) % 12) + 12) % 12)
                .unwrap_or_else(|| i64::from(anchor.month0()));
            let target_day = month_days
                .first()
                .copied()
                .unwrap_or_else(|| i64::from(anchor.day()));
            for offset in (0..400).step_by(usize::try_from(interval).ok()?.max(1)) {
                let year = anchor.year() + offset;
                let Some(date) = Local
                    .with_ymd_and_hms(
                        year,
                        u32::try_from(target_month + 1).ok()?,
                        u32::try_from(target_day).ok()?,
                        0,
                        0,
                        0,
                    )
                    .earliest()
                else {
                    continue;
                };
                if i64::from(date.month0()) != target_month {
                    continue;
                }
                let candidate = at_time(date, hour, minute)?;
                if candidate > from {
                    return Some(candidate);
                }
            }
            None
        }
        _ => None,
    }
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

/// `delayMinutes` → 一次性固定日历 cron + 带秒级 anchor 的 scheduleRule。
pub fn build_relative_delay_schedule(delay_minutes: i64, from: i64) -> (String, Value) {
    let target = local(from + delay_minutes * 60 * 1_000)
        .unwrap_or_else(|| local(from).unwrap_or_else(|| Local.timestamp_millis_opt(0).unwrap()));
    (
        format!(
            "{} {} {} {} *",
            target.minute(),
            target.hour(),
            target.day(),
            target.month()
        ),
        json!({
            "unit": "minute",
            "interval": delay_minutes,
            "hour": target.hour(),
            "minute": target.minute(),
            "anchorAt": from,
        }),
    )
}

/// `*/N * * * *` → 带创建时刻锚点的分钟规则。
pub fn infer_minute_interval_schedule_rule(cron_expr: &str, anchor_at: i64) -> Option<Value> {
    let trimmed = cron_expr.trim();
    let rest = trimmed.strip_prefix("*/")?;
    let mut parts = rest.split_whitespace();
    let interval: i64 = parts.next()?.parse().ok()?;
    if interval < 1 || parts.next()? != "*" || parts.next()? != "*" || parts.next()? != "*" {
        return None;
    }
    if parts.next()? != "*" || parts.next().is_some() {
        return None;
    }
    let anchor = local(anchor_at)?;
    Some(json!({
        "unit": "minute",
        "interval": interval,
        "hour": anchor.hour(),
        "minute": anchor.minute(),
        "anchorAt": anchor_at,
    }))
}

fn parse_cron_fields(cron_expr: &str) -> Map<String, Value> {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    let mut fields = Map::new();
    if parts.len() < 5 {
        return fields;
    }
    let parse_single = |token: &str| -> Option<i64> {
        if token == "*" || token == "?" {
            return None;
        }
        token.parse::<i64>().ok()
    };
    let parse_list = |token: &str| -> Option<Value> {
        if token == "*" || token == "?" {
            return None;
        }
        let values: Vec<i64> = token
            .split(',')
            .filter_map(|piece| piece.parse::<i64>().ok())
            .collect();
        (!values.is_empty()).then(|| json!(values))
    };
    if let Some(minute) = parse_single(parts[0]) {
        fields.insert("minute".to_owned(), json!(minute));
    }
    if let Some(hour) = parse_single(parts[1]) {
        fields.insert("hour".to_owned(), json!(hour));
    }
    if let Some(days) = parse_list(parts[2]) {
        fields.insert("monthDays".to_owned(), days);
    }
    if let Some(months) = parse_list(parts[3]) {
        fields.insert("months".to_owned(), months);
    }
    if let Some(weekdays) = parse_list(parts[4]) {
        fields.insert("weekdays".to_owned(), weekdays);
    }
    fields
}

/// 会话侧「每 N 单位」carrier → 权威 scheduleRule（TS `buildIntervalScheduleRule`）。
pub fn build_interval_schedule_rule(
    unit: &str,
    interval: i64,
    cron_expr: &str,
    anchor_at: i64,
) -> Result<Value, String> {
    let fields = parse_cron_fields(cron_expr);
    let anchor = local(anchor_at).ok_or_else(|| "invalid anchorAt".to_owned())?;
    let minute = fields
        .get("minute")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| i64::from(anchor.minute()));
    let hour = fields
        .get("hour")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| i64::from(anchor.hour()));
    let month_days = fields
        .get("monthDays")
        .cloned()
        .unwrap_or_else(|| json!([anchor.day()]));
    let months = fields
        .get("months")
        .cloned()
        .unwrap_or_else(|| json!([anchor.month()]));
    let weekdays = fields
        .get("weekdays")
        .cloned()
        .unwrap_or_else(|| json!([1]));
    let mut rule = Map::new();
    match unit {
        "minute" => {
            rule.insert("unit".to_owned(), json!("minute"));
            rule.insert("interval".to_owned(), json!(interval));
            rule.insert("hour".to_owned(), json!(anchor.hour()));
            rule.insert("minute".to_owned(), json!(anchor.minute()));
        }
        "hourly" => {
            rule.insert("unit".to_owned(), json!("hourly"));
            rule.insert("interval".to_owned(), json!(interval));
            rule.insert("hour".to_owned(), json!(0));
            rule.insert("minute".to_owned(), json!(minute));
        }
        "daily" => {
            rule.insert("unit".to_owned(), json!("daily"));
            rule.insert("interval".to_owned(), json!(interval));
            rule.insert("hour".to_owned(), json!(hour));
            rule.insert("minute".to_owned(), json!(minute));
        }
        "weekly" => {
            rule.insert("unit".to_owned(), json!("weekly"));
            rule.insert("interval".to_owned(), json!(interval));
            rule.insert("hour".to_owned(), json!(hour));
            rule.insert("minute".to_owned(), json!(minute));
            rule.insert("weekdays".to_owned(), weekdays);
        }
        "monthly" => {
            rule.insert("unit".to_owned(), json!("monthly"));
            rule.insert("interval".to_owned(), json!(interval));
            rule.insert("hour".to_owned(), json!(hour));
            rule.insert("minute".to_owned(), json!(minute));
            rule.insert("monthDays".to_owned(), month_days);
            rule.insert("monthlyMode".to_owned(), json!("date"));
        }
        "yearly" => {
            rule.insert("unit".to_owned(), json!("yearly"));
            rule.insert("interval".to_owned(), json!(interval));
            rule.insert("hour".to_owned(), json!(hour));
            rule.insert("minute".to_owned(), json!(minute));
            rule.insert("months".to_owned(), months);
            rule.insert("monthDays".to_owned(), month_days);
        }
        other => return Err(format!("Unsupported intervalUnit: {other}")),
    }
    rule.insert("anchorAt".to_owned(), json!(anchor_at));
    Ok(Value::Object(rule))
}

/// TS `scheduleRuleDefinition`：忽略 anchorAt，用于判断是否只有展示字段变化。
pub fn schedule_rule_definition(rule: &Value) -> String {
    let sorted = |key: &str| -> Option<Vec<i64>> {
        let mut values: Vec<i64> = rule[key]
            .as_array()?
            .iter()
            .filter_map(Value::as_i64)
            .collect();
        values.sort_unstable();
        Some(values)
    };
    json!([
        rule["unit"],
        rule["interval"],
        rule["hour"],
        rule["minute"],
        sorted("weekdays"),
        sorted("monthDays"),
        sorted("months"),
        rule["monthlyMode"],
    ])
    .to_string()
}

#[cfg(test)]
mod tests {
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
