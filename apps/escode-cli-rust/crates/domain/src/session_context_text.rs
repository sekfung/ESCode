//! ReadSessionContext 用到的 JS 字符串语义：UTF-16 长度与切片、`truncateText`、`toISOString`，
//! 以及保持键顺序的 `JSON.stringify`（工具 input 的原始顺序，serde_json 默认会排序）。

pub fn len16(value: &str) -> usize {
    value.encode_utf16().count()
}

/// JS `slice(start, end)`（UTF-16 码元）；切开代理对时以 U+FFFD 代替。
pub fn slice16(value: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    let end = end.min(units.len());
    let start = start.min(end);
    String::from_utf16_lossy(&units[start..end])
}

/// JS 字符串 `slice(-n)`。
#[cfg(test)]
pub fn tail16(value: &str, count: usize) -> String {
    let len = len16(value);
    slice16(value, len.saturating_sub(count), len)
}

/// TS `truncateText`。
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if len16(text) <= max_chars {
        return text.to_owned();
    }
    if max_chars <= 20 {
        return slice16(text, 0, max_chars);
    }
    format!("{}\n...[truncated]", slice16(text, 0, max_chars - 18))
}

/// JS `new Date(ms).toISOString()`（公历、毫秒、UTC）。
pub fn iso_time(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    // Howard Hinnant civil_from_days。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

/// 解析 JSON 文本并按原键顺序重新输出（JS `JSON.stringify(JSON.parse(raw))`）；非法 JSON 返回 None。
pub fn stringify_in_order(raw: &str) -> Option<String> {
    crate::json_order::Json::parse(raw).map(|value| value.compact())
}
