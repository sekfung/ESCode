//! JSON Schema 子集校验器（rust-unified-host.md P1 #10：`v4/command` schema 校验）。
//!
//! 支持从 zod `z.toJSONSchema` 生成的语料实际用到的关键字：`type`（含联合数组）、
//! `const`、`enum`、`properties`、`required`、`additionalProperties`、`items`、
//! `anyOf`/`oneOf`/`allOf`、`minimum`/`maximum`、`exclusiveMinimum`、
//! `minLength`/`maxLength`、`minItems`/`maxItems`。
//!
//! 不支持的关键字（`pattern`、`format`、`$ref`、`not` 等）在本子集里**不做判定**：
//! 校验器只报告它确知的违规，绝不因为"看不懂"而拒绝——保守方向与调用方一致
//! （调用方当前只把违规写日志，不改变线上应答，见 engine_requests.rs）。

use serde_json::Value;

/// 一条违规：JSON 路径 + 人类可读原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub path: String,
    pub message: String,
}

impl Violation {
    fn new(path: &str, message: impl Into<String>) -> Self {
        Self {
            path: if path.is_empty() {
                "$".to_owned()
            } else {
                path.to_owned()
            },
            message: message.into(),
        }
    }
}

/// 校验 `instance` 是否符合 `schema`；返回空 vec 表示通过。
pub fn validate(schema: &Value, instance: &Value) -> Vec<Violation> {
    let mut violations = Vec::new();
    check(schema, instance, "$", &mut violations);
    violations
}

fn check(schema: &Value, instance: &Value, path: &str, out: &mut Vec<Violation>) {
    let Some(schema) = schema.as_object() else {
        return;
    };
    // `true` / `{}` 形式：任何实例都通过。
    if schema.is_empty() {
        return;
    }
    if let Some(kind) = schema.get("type")
        && !type_matches(kind, instance)
    {
        out.push(Violation::new(
            path,
            format!(
                "expected {}, got {}",
                describe_type(kind),
                json_type(instance)
            ),
        ));
        // 类型不符时不再深入（子项判定只会产生噪声）。
        return;
    }
    if let Some(constant) = schema.get("const")
        && instance != constant
    {
        out.push(Violation::new(
            path,
            format!("expected const {constant}, got {instance}"),
        ));
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(instance)
    {
        out.push(Violation::new(
            path,
            format!("value {instance} not in enum {values:?}"),
        ));
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
        && instance.as_f64().is_some_and(|value| value < min)
    {
        out.push(Violation::new(path, format!("must be >= {min}")));
    }
    if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
        && instance.as_f64().is_some_and(|value| value > max)
    {
        out.push(Violation::new(path, format!("must be <= {max}")));
    }
    if let Some(exclusive) = schema.get("exclusiveMinimum").and_then(Value::as_f64)
        && instance.as_f64().is_some_and(|value| value <= exclusive)
    {
        out.push(Violation::new(path, format!("must be > {exclusive}")));
    }
    if let Some(text) = instance.as_str() {
        // JS 长度按 UTF-16 码元计；Rust 侧同口径以避免与非 BMP 字符上的漂移。
        let length = text.encode_utf16().count() as u64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
            && length < min
        {
            out.push(Violation::new(path, format!("length {length} < {min}")));
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
            && length > max
        {
            out.push(Violation::new(path, format!("length {length} > {max}")));
        }
    }
    if let Some(items) = instance.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            out.push(Violation::new(
                path,
                format!("items {} < {min}", items.len()),
            ));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            out.push(Violation::new(
                path,
                format!("items {} > {max}", items.len()),
            ));
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                check(item_schema, item, &format!("{path}[{index}]"), out);
            }
        }
    }
    if let Some(object) = instance.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    out.push(Violation::new(
                        &format!("{path}.{key}"),
                        "required property is missing",
                    ));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, value) in object {
                if let Some(child) = properties.get(key) {
                    check(child, value, &format!("{path}.{key}"), out);
                }
            }
        }
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => {
                let properties = schema.get("properties").and_then(Value::as_object);
                for key in object.keys() {
                    let known = properties.is_some_and(|map| map.contains_key(key));
                    if !known {
                        out.push(Violation::new(
                            &format!("{path}.{key}"),
                            "additional property is not allowed",
                        ));
                    }
                }
            }
            Some(Value::Object(_)) => {
                let properties = schema.get("properties").and_then(Value::as_object);
                let additional = schema.get("additionalProperties").cloned();
                for (key, value) in object {
                    let known = properties.is_some_and(|map| map.contains_key(key));
                    if !known && let Some(additional) = &additional {
                        check(additional, value, &format!("{path}.{key}"), out);
                    }
                }
            }
            _ => {}
        }
    }
    for keyword in ["allOf"] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                check(branch, instance, path, out);
            }
        }
    }
    for keyword in ["anyOf", "oneOf"] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array)
            && !branches.is_empty()
        {
            let mut matched = 0_usize;
            let mut branch_violations = Vec::new();
            for branch in branches {
                let mut local = Vec::new();
                check(branch, instance, path, &mut local);
                if local.is_empty() {
                    matched += 1;
                } else {
                    branch_violations.push(local);
                }
            }
            // anyOf / oneOf 都要求至少一支匹配（oneOf 的"恰好一支"在语料里未出现，
            // 且 zod 的 union 语义就是"至少一支"）。
            if matched == 0 {
                // 报告最接近的一支（违规最少），避免把整棵联合的路径都倒出来。
                let closest = branch_violations
                    .into_iter()
                    .min_by_key(Vec::len)
                    .unwrap_or_else(|| vec![Violation::new(path, "no branch matched")]);
                out.extend(closest);
            }
        }
    }
}

fn json_type(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(kind: &Value, instance: &Value) -> bool {
    match kind {
        Value::String(name) => single_type_matches(name, instance),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| single_type_matches(name, instance)),
        _ => true,
    }
}

fn single_type_matches(name: &str, instance: &Value) -> bool {
    match name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        "number" => instance.is_number(),
        // JSON Schema 的 integer 包含整数值的浮点（如 1.0），与 zod 的 z.number().int() 同口径。
        "integer" => instance.as_f64().is_some_and(|value| value.fract() == 0.0),
        _ => true,
    }
}

fn describe_type(kind: &Value) -> String {
    match kind {
        Value::String(name) => name.clone(),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" | "),
        _ => "any".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn required_and_types_are_enforced() {
        let schema = json!({
            "type": "object",
            "required": ["commandId", "issuedAt"],
            "properties": {
                "commandId": {"type": "string", "minLength": 1},
                "issuedAt": {"type": "number"},
                "sessionId": {"type": ["string", "null"]}
            }
        });
        let violations = validate(&schema, &json!({"commandId": "", "sessionId": null}));
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(violations.iter().any(|v| v.path == "$.commandId"));
        assert!(violations.iter().any(|v| v.path == "$.issuedAt"));
        assert!(validate(&schema, &json!({"commandId": "c", "issuedAt": 1.5})).is_empty());
        assert!(
            validate(&schema, &json!({"commandId": "c", "issuedAt": null}))
                .iter()
                .any(|v| v.message.contains("expected number, got null"))
        );
    }

    #[test]
    fn enum_const_and_union_matching() {
        let schema = json!({
            "anyOf": [
                {"type": "object", "required": ["text"], "properties": {"text": {"type": "string"}}},
                {"type": "object", "required": ["count"], "properties": {"count": {"type": "integer"}}}
            ]
        });
        assert!(validate(&schema, &json!({"text": "hi"})).is_empty());
        assert!(validate(&schema, &json!({"count": 3})).is_empty());
        // 最近的一支决定报告路径（避免把两棵分支的路径都倒出来）；同分取先声明的一支。
        let violations = validate(&schema, &json!({"count": "3"}));
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            matches!(violations[0].path.as_str(), "$.count" | "$.text"),
            "{violations:?}"
        );
        let enum_schema = json!({"enum": ["a", "b"]});
        assert!(validate(&enum_schema, &json!("b")).is_empty());
        assert_eq!(validate(&enum_schema, &json!("c")).len(), 1);
        let const_schema = json!({"const": "session"});
        assert!(validate(&const_schema, &json!("session")).is_empty());
        assert_eq!(validate(&const_schema, &json!("other")).len(), 1);
    }

    #[test]
    fn arrays_and_additional_properties() {
        let schema = json!({
            "type": "object",
            "properties": {"items": {"type": "array", "items": {"type": "string"}, "minItems": 1}},
            "additionalProperties": false
        });
        assert!(validate(&schema, &json!({"items": ["a"]})).is_empty());
        let violations = validate(&schema, &json!({"items": [1], "extra": true}));
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(violations.iter().any(|v| v.path == "$.items[0]"));
        assert!(violations.iter().any(|v| v.path == "$.extra"));
        assert!(
            validate(&schema, &json!({"items": []}))[0]
                .message
                .contains("items 0 < 1")
        );
    }

    #[test]
    fn unsupported_keywords_never_reject() {
        // `pattern`/`format` 不在子集内：不判定，也不因此报告违规。
        let schema = json!({"type": "string", "pattern": "^[0-9]+$", "format": "uuid"});
        assert!(validate(&schema, &json!("not-matching")).is_empty());
        // 空 schema（zod 的 z.unknown()）与布尔型 schema 都放行。
        assert!(validate(&json!({}), &json!({"anything": true})).is_empty());
        assert!(validate(&json!(true), &json!(null)).is_empty());
    }
}
