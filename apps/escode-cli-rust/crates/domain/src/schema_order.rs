//! 工具定义的属性顺序（docs/specs/rust-tool-schema-order.md）：serde_json 排序键，模型请求与入参校验里的工具
//! schema 按登记的声明顺序使用。登记表是按排序文本内容寻址的进程级缓存（只存 schema 文本，无 IO，重复登记幂等）。
use crate::json_order::Json;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

/// 远超单进程可能的不同 schema 数；超出时整表清空而不是无界增长（查不到只会回落为排序输出）。
const LIMIT: usize = 20_000;

fn table() -> &'static Mutex<HashMap<String, String>> {
    static TABLE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// 登记一份声明顺序的 schema（排序与声明顺序相同时无需登记）。
pub fn remember(ordered: &Json) {
    let Some((sorted, ordered)) = entry(ordered) else {
        return;
    };
    let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
    if table.len() >= LIMIT && !table.contains_key(&sorted) {
        table.clear();
    }
    table.insert(sorted, ordered);
}

/// 按排序文本查声明顺序的文本。
pub fn lookup(sorted: &str) -> Option<String> {
    table().lock().unwrap_or_else(|e| e.into_inner()).get(sorted).cloned()
}

/// schema 的声明顺序形式：有登记时用登记文本，否则为排序形式。
pub fn ordered(schema: &Value) -> Option<Json> {
    let sorted = serde_json::to_string(schema).ok()?;
    Json::parse(&lookup(&sorted).unwrap_or(sorted))
}

/// 由原文顺序的 schema 生成登记项：（排序后的紧凑文本，JS `JSON.stringify` 规则的保序紧凑文本）。
/// 排序文本由同一份数值直接转换得到，与请求体中 serde_json 的序列化逐字一致。
pub fn entry(ordered: &Json) -> Option<(String, String)> {
    let sorted = serde_json::to_string(&to_value(ordered)).ok()?;
    let ordered = ordered.compact();
    (sorted != ordered).then_some((sorted, ordered))
}

/// 模型请求体编码：工具 schema 查到登记时换成保序文本，否则与 `serde_json::to_vec` 逐字相同。
pub fn encode(body: &Value, lookup: impl Fn(&str) -> Option<String>) -> String {
    let plain = || serde_json::to_string(body).unwrap_or_default();
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return plain();
    };
    let mut changed = false;
    let rendered: Vec<String> = tools
        .iter()
        .map(|tool| {
            let text = serde_json::to_string(tool).unwrap_or_default();
            let schema = tool
                .pointer("/function/parameters")
                .or_else(|| tool.get("parameters"))
                .or_else(|| tool.get("input_schema"));
            let Some(sorted) = schema.and_then(|s| serde_json::to_string(s).ok()) else {
                return text;
            };
            match lookup(&sorted) {
                Some(ordered) if text.contains(&sorted) => {
                    changed = true;
                    text.replacen(&sorted, &ordered, 1)
                }
                _ => text,
            }
        })
        .collect();
    if !changed {
        return plain();
    }
    // 排序后 "tools" 之后可能还有键（如 "top_p"）：按原顺序逐键输出，只替换 tools 的值。
    let Some(object) = body.as_object() else {
        return plain();
    };
    let fields: Vec<String> = object
        .iter()
        .map(|(key, value)| {
            let value = if key == "tools" {
                format!("[{}]", rendered.join(","))
            } else {
                serde_json::to_string(value).unwrap_or_default()
            };
            format!("{}:{value}", serde_json::to_string(key).unwrap_or_default())
        })
        .collect();
    format!("{{{}}}", fields.join(","))
}

fn to_value(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(n.clone()),
        Json::String(s) => Value::String(s.clone()),
        Json::Array(items) => Value::Array(items.iter().map(to_value).collect()),
        Json::Object(entries) => Value::Object(entries.iter().map(|(k, v)| (k.clone(), to_value(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splices_registered_schema_and_keeps_other_bytes() {
        let raw = Json::parse(r#"{"type":"object","properties":{"text":{"type":"string"},"count":{"type":"integer","minimum":1.0}},"required":["text"]}"#).unwrap();
        let (sorted, ordered) = entry(&raw).unwrap();
        assert_eq!(ordered, r#"{"type":"object","properties":{"text":{"type":"string"},"count":{"type":"integer","minimum":1}},"required":["text"]}"#);
        let schema: Value = serde_json::from_str(&sorted).unwrap();
        let body = json!({"model": "m", "tools": [{"type": "function", "function": {"name": "mcp__a__b", "parameters": schema}}], "top_p": 1});
        let lookup = |key: &str| (key == sorted).then(|| ordered.clone());
        // 保序 schema 按 JS 规则输出数值（1.0 → 1，与 Node 请求相同）；其余键与 serde_json 输出一致。
        assert_eq!(
            encode(&body, lookup),
            format!(r#"{{"model":"m","tools":[{{"function":{{"name":"mcp__a__b","parameters":{ordered}}},"type":"function"}}],"top_p":1}}"#)
        );
        assert_eq!(encode(&body, |_| None), serde_json::to_string(&body).unwrap());
    }

    #[test]
    fn already_sorted_schema_needs_no_entry() {
        assert_eq!(entry(&Json::parse(r#"{"a":1,"b":2}"#).unwrap()), None);
    }
}
