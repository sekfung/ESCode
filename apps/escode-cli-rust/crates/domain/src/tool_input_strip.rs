//! 内置工具入参的未知键丢弃（docs/specs/rust-tool-input-validation.md）：TS runtime schema（zod v3）的非 strict
//! object 会丢弃未声明的键。位置由 zod schema 遍历实测得出；键是否声明以 JSON Schema 的 properties 为准。
use crate::json_order::Json;

/// 工具名 → 非 strict object 的位置（`""` 为顶层，`*` 为数组元素）。
const PATHS: [(&str, &str); 10] = [
    ("Agent", ""),
    ("Edit", ""),
    ("ExitPlanMode", ""),
    ("Glob", ""),
    ("Grep", ""),
    ("Read", ""),
    ("Skill", ""),
    ("TodoWrite", "todos/*"),
    ("WebFetch", ""),
    ("Write", ""),
];

/// runtime schema 认识、但发给模型的定义里可能没有的顶层键：zod 不丢弃，随后按定义校验报多余参数。
/// Read 的 runtime schema 总含 `pages`，模型不支持 PDF 时定义里没有它（差分发现：之前 Rust 丢弃后照常读取）。
const RUNTIME_KEYS: &[(&str, &[&str])] = &[("Read", &["pages"])];

/// 按 zod 语义丢弃未知键；没有需要丢弃的键时返回原值的克隆。
pub fn strip(tool: &str, value: &Json, schema: &Json) -> Json {
    let mut value = value.clone();
    let runtime = RUNTIME_KEYS.iter().find(|(name, _)| *name == tool).map_or(&[][..], |(_, keys)| *keys);
    for (_, path) in PATHS.iter().filter(|(name, _)| *name == tool) {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let keep = if segments.is_empty() { runtime } else { &[] };
        strip_at(&mut value, schema, &segments, keep);
    }
    value
}

fn strip_at(value: &mut Json, schema: &Json, path: &[&str], keep: &[&str]) {
    match path.split_first() {
        None => {
            // schema 未声明 properties 时无从判断哪些键未知，保持原样。
            let (Json::Object(entries), Some(Json::Object(properties))) = (value, schema.get("properties")) else {
                return;
            };
            entries.retain(|(key, _)| keep.contains(&key.as_str()) || properties.iter().any(|(k, _)| k == key));
        }
        Some((&"*", rest)) => {
            let (Json::Array(items), Some(item_schema)) = (value, schema.get("items")) else {
                return;
            };
            items.iter_mut().for_each(|item| strip_at(item, item_schema, rest, keep));
        }
        Some((key, rest)) => {
            let (Some(child), Some(child_schema)) = (
                value.get_mut(key),
                schema.get("properties").and_then(|p| p.get(key)),
            ) else {
                return;
            };
            strip_at(child, child_schema, rest, keep);
        }
    }
}

/// TS zod `.default()`：缺失且 schema 声明了 `default` 的属性先填入默认值再做 JSON 校验
/// （TS 的问题投影同样不把有默认值的缺失属性报给模型）。只用于内置工具；MCP 工具没有 runtime schema。
pub fn fill_defaults(value: &mut Json, schema: &Json) {
    match (value, schema.get("properties"), schema.get("items")) {
        (Json::Object(entries), Some(Json::Object(properties)), _) => {
            for (key, property) in properties {
                match entries.iter_mut().find(|(k, _)| k == key) {
                    Some((_, child)) => fill_defaults(child, property),
                    None => {
                        if let Some(default) = property.get("default") {
                            entries.push((key.clone(), default.clone()));
                        }
                    }
                }
            }
        }
        (Json::Array(items), _, Some(item_schema)) => items.iter_mut().for_each(|item| fill_defaults(item, item_schema)),
        _ => {}
    }
}

/// TS runtime schema 的 preprocess/transform（zod 实测）：在 JSON 校验之前把常见的宽松写法转成声明类型。
/// - Bash `timeout`：数字字符串转数字；Bash `run_in_background`/`dangerouslyDisableSandbox` 与 Edit `replace_all`：
///   true/1/yes/y/on 与 false/0/no/n/off（不区分大小写）及数字 1/0 转布尔；
/// - TaskOutput `block`：只认 "true"/"false"；
/// - Skill：旧写法 `{name, args}`（无 `skill`）转为 `{skill, args}`。
pub fn coerce(tool: &str, value: &Json) -> Json {
    let mut value = value.clone();
    let Json::Object(entries) = &mut value else {
        return value;
    };
    let mut apply = |key: &str, f: fn(&Json) -> Option<Json>| {
        if let Some((_, field)) = entries.iter_mut().find(|(k, _)| k == key)
            && let Some(converted) = f(field)
        {
            *field = converted;
        }
    };
    match tool {
        "Bash" => {
            apply("timeout", semantic_number);
            apply("run_in_background", semantic_boolean);
            apply("dangerouslyDisableSandbox", semantic_boolean);
        }
        "Edit" => apply("replace_all", semantic_boolean),
        "TaskOutput" => apply("block", |v| match v.as_str() {
            Some("true") => Some(Json::Bool(true)),
            Some("false") => Some(Json::Bool(false)),
            _ => None,
        }),
        "Skill" if !entries.iter().any(|(k, _)| k == "skill") => {
            let name = entries.iter().find(|(k, _)| k == "name").and_then(|(_, v)| v.as_str()).filter(|n| !n.is_empty());
            if let Some(name) = name.map(str::to_owned) {
                let args = entries.iter().find(|(k, _)| k == "args").map(|(_, v)| v.clone());
                entries.clear();
                if let Some(args) = args {
                    entries.push(("args".into(), args));
                }
                entries.push(("skill".into(), Json::String(name)));
            }
        }
        _ => {}
    }
    value
}

/// TS `semanticNumber`：去空白后非空、`Number()` 为有限数时转换。
fn semantic_number(value: &Json) -> Option<Json> {
    let trimmed = value.as_str()?.trim();
    let parsed: f64 = trimmed.parse().ok().filter(|n: &f64| n.is_finite() && !trimmed.is_empty())?;
    serde_json::Number::from_f64(parsed).map(Json::Number)
}

/// TS Bash/Edit 的 `semanticBoolean`。
fn semantic_boolean(value: &Json) -> Option<Json> {
    match value {
        Json::Number(n) if n.as_f64() == Some(1.0) => Some(Json::Bool(true)),
        Json::Number(n) if n.as_f64() == Some(0.0) => Some(Json::Bool(false)),
        Json::String(text) => match text.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "y" | "on" => Some(Json::Bool(true)),
            "false" | "0" | "no" | "n" | "off" => Some(Json::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_top_level_and_nested_array_items() {
        let schema = Json::parse(r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string"}}}}}}"#).unwrap();
        let input = Json::parse(r#"{"todos":[{"content":"a","unused":true}],"extra":1}"#).unwrap();
        assert_eq!(strip("TodoWrite", &input, &schema).compact(), r#"{"todos":[{"content":"a"}],"extra":1}"#);
        let read = Json::parse(r#"{"type":"object","properties":{"file_path":{"type":"string"}}}"#).unwrap();
        let input = Json::parse(r#"{"file_path":"a","bogus":1}"#).unwrap();
        assert_eq!(strip("Read", &input, &read).compact(), r#"{"file_path":"a"}"#);
        assert_eq!(strip("Bash", &input, &read), input);
        assert_eq!(strip("Read", &input, &Json::parse(r#"{"type":"object"}"#).unwrap()), input);
        // runtime schema 认识的 `pages` 不因定义里缺失而丢弃。
        let pdf = Json::parse(r#"{"file_path":"a","pages":"1","bogus":1}"#).unwrap();
        assert_eq!(strip("Read", &pdf, &read).compact(), r#"{"file_path":"a","pages":"1"}"#);
    }

    #[test]
    fn coerces_like_ts_runtime_schemas() {
        let bash = Json::parse(r#"{"command":"x","timeout":" 5000 ","run_in_background":"Yes","dangerouslyDisableSandbox":0}"#).unwrap();
        assert_eq!(coerce("Bash", &bash).compact(), r#"{"command":"x","timeout":5000,"run_in_background":true,"dangerouslyDisableSandbox":false}"#);
        let bad = Json::parse(r#"{"command":"x","timeout":"abc","run_in_background":"maybe"}"#).unwrap();
        assert_eq!(coerce("Bash", &bad), bad);
        let task = Json::parse(r#"{"task_id":"t","block":"false"}"#).unwrap();
        assert_eq!(coerce("TaskOutput", &task).compact(), r#"{"task_id":"t","block":false}"#);
        let legacy = Json::parse(r#"{"name":"pdf","args":"x"}"#).unwrap();
        assert_eq!(coerce("Skill", &legacy).compact(), r#"{"args":"x","skill":"pdf"}"#);
        let current = Json::parse(r#"{"skill":"pdf","name":"ignored"}"#).unwrap();
        assert_eq!(coerce("Skill", &current), current);
    }

    #[test]
    fn fills_nested_defaults_only_when_missing() {
        let schema = Json::parse(r#"{"type":"object","properties":{"questions":{"type":"array","items":{"type":"object","properties":{"multiSelect":{"type":"boolean","default":false}}}}}}"#).unwrap();
        let mut value = Json::parse(r#"{"questions":[{},{"multiSelect":true}]}"#).unwrap();
        fill_defaults(&mut value, &schema);
        assert_eq!(value.compact(), r#"{"questions":[{"multiSelect":false},{"multiSelect":true}]}"#);
    }
}
