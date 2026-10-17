//! Claude 日志时间戳解析（TS `toTimestampMs`）：秒/毫秒数字与 RFC3339。

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

pub(super) fn to_timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_f64() {
        if number > 1_000_000_000_000.0 {
            return Some(number as i64);
        }
        if number > 1_000_000_000.0 {
            return Some((number * 1000.0) as i64);
        }
        return None;
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(number) = text.parse::<f64>() {
        return to_timestamp_ms(&Value::from(number));
    }
    parse_rfc3339_ms(text)
}

fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    static ISO: OnceLock<Regex> = OnceLock::new();
    let regex = ISO.get_or_init(|| {
        Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(Z|z|[+-]\d{2}:\d{2})$",
        )
        .expect("regex")
    });
    let captures = regex.captures(text)?;
    let number = |index: usize| {
        captures
            .get(index)
            .and_then(|m| m.as_str().parse::<i64>().ok())
    };
    let (year, month, day) = (number(1)?, number(2)?, number(3)?);
    let (hour, minute, second) = (number(4)?, number(5)?, number(6)?);
    let millis = captures
        .get(7)
        .map(|m| {
            let padded = format!("{:0<3}", m.as_str());
            padded[..3].parse::<i64>().unwrap_or(0)
        })
        .unwrap_or(0);
    let offset_minutes = match captures.get(8).map(|m| m.as_str()) {
        Some("Z" | "z") => 0,
        Some(offset) => {
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let hours = offset[1..3].parse::<i64>().ok()?;
            let minutes = offset[4..6].parse::<i64>().ok()?;
            sign * (hours * 60 + minutes)
        }
        None => 0,
    };
    let days = days_from_civil(year, month, day);
    Some(
        ((days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60) * 1000)
            + millis,
    )
}

/// Howard Hinnant 的 days-from-civil。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
