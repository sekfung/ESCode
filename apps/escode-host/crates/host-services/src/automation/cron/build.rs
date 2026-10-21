//! scheduleRule 的构造与稳定序列化（TS `buildIntervalScheduleRule` / `scheduleRuleDefinition`）。

use chrono::{Datelike, Local, TimeZone, Timelike};
use serde_json::{Map, Value, json};

use super::local;

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
