//! ReadSessionContext 用到的 JS 字符串语义：UTF-16 长度与切片、`truncateText`、`toISOString`，
//! 以及保持键顺序的 `JSON.stringify`（工具 input 的原始顺序，serde_json 默认会排序）。

use std::fmt::Write as _;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

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
    let value: Ordered = serde_json::from_str(raw).ok()?;
    let mut out = String::new();
    value.write(&mut out);
    Some(out)
}

enum Ordered {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl Ordered {
    fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => {
                let _ = write!(out, "{value}");
            }
            Self::String(value) => out.push_str(&serde_json::to_string(value).unwrap_or_default()),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).unwrap_or_default());
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Ordered, E> {
                Ok(Ordered::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Ordered, E> {
                Ok(Ordered::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Ordered, E> {
                Ok(serde_json::Number::from_f64(v).map_or(Ordered::Null, Ordered::Number))
            }
            fn visit_str<E>(self, v: &str) -> Result<Ordered, E> {
                Ok(Ordered::String(v.to_owned()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                // JS：重复键保留首次出现的位置、最后一次的值。
                let mut entries: Vec<(String, Ordered)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Ordered>()? {
                    match entries.iter_mut().find(|(k, _)| *k == key) {
                        Some(slot) => slot.1 = value,
                        None => entries.push((key, value)),
                    }
                }
                Ok(Ordered::Object(entries))
            }
        }
        deserializer.deserialize_any(V)
    }
}
