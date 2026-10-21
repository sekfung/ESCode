//! automation 写入参数校验（mode / cron / scheduleRule）。

use serde_json::Value;

use super::super::cron;

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

pub(super) fn validate_mode(mode: Option<&str>) -> Result<(), String> {
    if let Some(mode) = mode
        && !mode.is_empty()
        && !MODES.contains(&mode)
    {
        return Err(format!("非法的 mode：{mode}"));
    }
    Ok(())
}

pub(super) fn validate_schedule_rule(rule: &Value) -> Result<(), String> {
    let unit = rule["unit"]
        .as_str()
        .ok_or_else(|| "调度规则的 unit 不受支持".to_owned())?;
    if !matches!(
        unit,
        "minute" | "hourly" | "daily" | "weekly" | "monthly" | "yearly"
    ) {
        return Err("调度规则的 unit 不受支持".to_owned());
    }
    let interval = rule["interval"]
        .as_i64()
        .filter(|interval| *interval >= 1)
        .ok_or_else(|| "调度规则的 interval 必须是正整数".to_owned())?;
    if unit == "monthly" && interval > 1_200 {
        return Err("monthly interval 不能超过 1200".to_owned());
    }
    if !(0..=23).contains(&rule["hour"].as_i64().unwrap_or(-1)) {
        return Err("调度规则的 hour 必须是 0-23 的整数".to_owned());
    }
    if !(0..=59).contains(&rule["minute"].as_i64().unwrap_or(-1)) {
        return Err("调度规则的 minute 必须是 0-59 的整数".to_owned());
    }
    if let Some(mode) = rule.get("monthlyMode").and_then(Value::as_str)
        && !matches!(mode, "date" | "weekday")
    {
        return Err("调度规则的 monthlyMode 不受支持".to_owned());
    }
    let list_ok = |key: &str, min: i64, max: i64| -> bool {
        rule.get(key)
            .and_then(Value::as_array)
            .is_some_and(|items| {
                !items.is_empty()
                    && items.iter().all(|item| {
                        item.as_i64()
                            .is_some_and(|value| (min..=max).contains(&value))
                    })
            })
    };
    if rule.get("weekdays").is_some() && !list_ok("weekdays", 0, 6) {
        return Err("调度规则的 weekdays 必须是 0-6 的非空整数数组".to_owned());
    }
    if unit == "weekly" && !list_ok("weekdays", 0, 6) {
        return Err("weekly 规则必须包含有效的 weekdays".to_owned());
    }
    if rule.get("monthDays").is_some() && !list_ok("monthDays", 1, 31) {
        return Err("调度规则的 monthDays 必须是 1-31 的非空整数数组".to_owned());
    }
    if rule.get("months").is_some() && !list_ok("months", 1, 12) {
        return Err("调度规则的 months 必须是 1-12 的非空整数数组".to_owned());
    }
    Ok(())
}

pub(super) fn validate_cron(cron_expr: &str, at: i64) -> Result<(), String> {
    if cron::compute_next_run_at(cron_expr, at).is_none() {
        return Err(format!("非法的 cron 表达式：{cron_expr}"));
    }
    Ok(())
}
