//! 自定义 scheduleRule 的下一次触发时间（TS `computeScheduleRuleNextRunAt`）。

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use serde_json::Value;

use super::local;

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
