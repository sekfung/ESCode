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

/// 按 zod 语义丢弃未知键；没有需要丢弃的键时返回原值的克隆。
pub fn strip(tool: &str, value: &Json, schema: &Json) -> Json {
    let mut value = value.clone();
    for (_, path) in PATHS.iter().filter(|(name, _)| *name == tool) {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        strip_at(&mut value, schema, &segments);
    }
    value
}

fn strip_at(value: &mut Json, schema: &Json, path: &[&str]) {
    match path.split_first() {
        None => {
            // schema 未声明 properties 时无从判断哪些键未知，保持原样。
            let (Json::Object(entries), Some(Json::Object(properties))) = (value, schema.get("properties")) else {
                return;
            };
            entries.retain(|(key, _)| properties.iter().any(|(k, _)| k == key));
        }
        Some((&"*", rest)) => {
            let (Json::Array(items), Some(item_schema)) = (value, schema.get("items")) else {
                return;
            };
            items.iter_mut().for_each(|item| strip_at(item, item_schema, rest));
        }
        Some((key, rest)) => {
            let (Some(child), Some(child_schema)) = (
                value.get_mut(key),
                schema.get("properties").and_then(|p| p.get(key)),
            ) else {
                return;
            };
            strip_at(child, child_schema, rest);
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
    }

    #[test]
    fn fills_nested_defaults_only_when_missing() {
        let schema = Json::parse(r#"{"type":"object","properties":{"questions":{"type":"array","items":{"type":"object","properties":{"multiSelect":{"type":"boolean","default":false}}}}}}"#).unwrap();
        let mut value = Json::parse(r#"{"questions":[{},{"multiSelect":true}]}"#).unwrap();
        fill_defaults(&mut value, &schema);
        assert_eq!(value.compact(), r#"{"questions":[{"multiSelect":false},{"multiSelect":true}]}"#);
    }
}
