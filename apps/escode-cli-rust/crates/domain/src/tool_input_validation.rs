//! 工具入参 JSON Schema 校验（docs/specs/rust-tool-input-validation.md），逐项对齐 TS
//! `core/src/tool/json-schema.ts`、`tool-input-validation-issues.ts` 与 `input-validation-model-content.ts`
//! 的 MCP 路径（无 runtime schema，问题即 JSON Schema 问题）。问题对象的字段顺序会进入模型可见的
//! JSON 回落文案，因此用保序的 `Json` 构造。
use crate::json_order::Json;

type Path = Vec<Json>;

/// `value` 为 `None` 表示 JS `undefined`（缺失的属性）。返回问题列表，空表示通过。
pub fn validate(value: &Json, schema: &Json) -> Vec<Json> {
    match schema {
        Json::Object(entries) if !entries.is_empty() => {
            let mut issues = vec![];
            node(Some(value), schema, &[], &mut issues);
            issues
        }
        _ => vec![],
    }
}

/// TS `createInitialInputValidationModelContent`（无 runtime issue 时即 JSON 问题本身）。
pub fn model_content(tool: &str, issues: &[Json]) -> String {
    format!("<tool_use_error>InputValidationError: {}</tool_use_error>", format_error(tool, issues))
}

fn node(value: Option<&Json>, schema: &Json, path: &[Json], issues: &mut Vec<Json>) {
    if let Some(candidates) = schema_array(schema.get("oneOf")) {
        let results: Vec<Vec<Json>> = candidates
            .iter()
            .map(|candidate| {
                let mut nested = vec![];
                node(value, candidate, &[], &mut nested);
                nested
            })
            .collect();
        // TS 以 legacy errors 判断是否匹配；每个失败都同时产生 issue，两者等价。
        if results.iter().filter(|r| r.is_empty()).count() != 1 {
            let errors = results.into_iter().map(Json::Array).collect();
            issues.push(object(vec![
                ("code", Json::str("invalid_union")),
                ("errors", Json::Array(errors)),
                ("path", Json::Array(path.to_vec())),
                ("message", Json::str("Invalid input")),
            ]));
        }
        return;
    }
    let mut value_failed = false;
    if let Some(constant) = schema.get("const")
        && !same(value, constant)
    {
        issues.push(invalid_value(std::slice::from_ref(constant), path));
        value_failed = true;
    }
    if let Some(values) = schema.get("enum").and_then(Json::as_array)
        && !values.iter().any(|v| same(value, v))
    {
        issues.push(invalid_value(values, path));
        value_failed = true;
    }
    if let Some(kind) = schema.get("type")
        && !matches_type(value, kind)
    {
        // 值约束已失败时只保留 invalid_value（TS 同）。
        if !value_failed {
            match kind {
                Json::Array(kinds) => {
                    let errors = kinds.iter().map(|k| Json::Array(vec![invalid_type(value, k, &[])])).collect();
                    issues.push(object(vec![
                        ("code", Json::str("invalid_union")),
                        ("errors", Json::Array(errors)),
                        ("path", Json::Array(path.to_vec())),
                        ("message", Json::str("Invalid input")),
                    ]));
                }
                _ => issues.push(invalid_type(value, kind, path)),
            }
        }
        return;
    }
    match value {
        Some(Json::String(text)) => {
            let length = text.encode_utf16().count() as f64;
            if let Some(min) = number(schema.get("minLength"))
                && length < min
            {
                issues.push(size("too_small", "string", schema.get("minLength").unwrap(), path));
            }
            if let Some(max) = number(schema.get("maxLength"))
                && length > max
            {
                issues.push(size("too_big", "string", schema.get("maxLength").unwrap(), path));
            }
        }
        Some(Json::Number(n)) => {
            let n = n.as_f64().unwrap_or(f64::NAN);
            if let Some(min) = number(schema.get("minimum"))
                && n < min
            {
                issues.push(size("too_small", "number", schema.get("minimum").unwrap(), path));
            }
            if let Some(max) = number(schema.get("maximum"))
                && n > max
            {
                issues.push(size("too_big", "number", schema.get("maximum").unwrap(), path));
            }
        }
        Some(Json::Array(items)) => array(items, schema, path, issues),
        Some(Json::Object(entries)) => record(entries, schema, path, issues),
        _ => {}
    }
}

fn array(items: &[Json], schema: &Json, path: &[Json], issues: &mut Vec<Json>) {
    let length = items.len() as f64;
    let minimum = number(schema.get("minItems"))
        .filter(|min| length < *min)
        .map(|_| size("too_small", "array", schema.get("minItems").unwrap(), path));
    let maximum = number(schema.get("maxItems"))
        .filter(|max| length > *max)
        .map(|_| size("too_big", "array", schema.get("maxItems").unwrap(), path));
    if let Some(item_schema) = schema.get("items").filter(|s| s.is_object()) {
        for (index, item) in items.iter().enumerate() {
            node(Some(item), item_schema, &child(path, Json::Number(index.into())), issues);
        }
    }
    // 元素问题先于数组自身的长度问题（TS 注释：provider-visible 顺序）。
    issues.extend(minimum);
    issues.extend(maximum);
}

fn record(entries: &[(String, Json)], schema: &Json, path: &[Json], issues: &mut Vec<Json>) {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Json::as_array)
        .map(|list| list.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default();
    let empty = vec![];
    let properties = match schema.get("properties") {
        Some(Json::Object(props)) => props,
        _ => &empty,
    };
    let present = |key: &str| entries.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    for (key, property) in properties {
        let key_path = child(path, Json::str(key.clone()));
        match present(key) {
            None if required.contains(&key.as_str()) => missing(property, &key_path, issues),
            None => {}
            Some(value) if property.is_object() => node(Some(value), property, &key_path, issues),
            Some(_) => {}
        }
    }
    for key in &required {
        if !properties.iter().any(|(k, _)| k == key) && present(key).is_none() {
            issues.push(invalid_type(None, &Json::str("unknown"), &child(path, Json::str(*key))));
        }
    }
    if matches!(schema.get("additionalProperties"), Some(Json::Bool(false))) {
        let unexpected: Vec<Json> = entries
            .iter()
            .filter(|(key, _)| !properties.iter().any(|(k, _)| k == key))
            .map(|(key, _)| Json::str(key.clone()))
            .collect();
        if !unexpected.is_empty() {
            let plural = if unexpected.len() > 1 { "s" } else { "" };
            let message = format!("Unrecognized key{plural}: {}", issue_values(&unexpected, ", "));
            issues.push(object(vec![
                ("code", Json::str("unrecognized_keys")),
                ("keys", Json::Array(unexpected)),
                ("path", Json::Array(path.to_vec())),
                ("message", Json::String(message)),
            ]));
        }
    }
}

/// TS `createMissingPropertyIssues`：先按属性 schema 校验 undefined，没有问题时回落 invalid_type。
fn missing(property: &Json, path: &[Json], issues: &mut Vec<Json>) {
    if property.is_object() {
        let mut nested = vec![];
        node(None, property, path, &mut nested);
        if !nested.is_empty() {
            issues.extend(nested);
            return;
        }
    }
    let inferred = match property {
        Json::Object(_) => match property.get("type") {
            Some(Json::String(kind)) => kind.clone(),
            _ if matches!(property.get("properties"), Some(Json::Object(_))) || property.get("required").and_then(Json::as_array).is_some() => "object".into(),
            _ if matches!(property.get("items"), Some(Json::Object(_))) => "array".into(),
            _ => "unknown".into(),
        },
        _ => "unknown".into(),
    };
    issues.push(invalid_type(None, &Json::String(inferred), path));
}

fn matches_type(value: Option<&Json>, kind: &Json) -> bool {
    match kind {
        Json::Array(kinds) => kinds.iter().any(|k| matches_type(value, k)),
        Json::String(kind) => match kind.as_str() {
            "array" => matches!(value, Some(Json::Array(_))),
            "boolean" => matches!(value, Some(Json::Bool(_))),
            "integer" => matches!(value, Some(Json::Number(n)) if n.as_f64().is_some_and(|f| f.fract() == 0.0)),
            "null" => matches!(value, Some(Json::Null)),
            "number" => matches!(value, Some(Json::Number(_))),
            "object" => matches!(value, Some(Json::Object(_))),
            "string" => matches!(value, Some(Json::String(_))),
            _ => true,
        },
        _ => true,
    }
}

/// TS `createInvalidTypeIssue`：integer 对有限数字报 `int`（format safeint）。
fn invalid_type(value: Option<&Json>, kind: &Json, path: &[Json]) -> Json {
    let kind = js_string(kind);
    let (expected, format) = match (kind.as_str(), value) {
        ("integer", Some(Json::Number(_))) => ("int".to_owned(), Some("safeint")),
        ("integer", _) => ("number".to_owned(), None),
        _ => (kind, None),
    };
    let message = format!("Invalid input: expected {expected}, received {}", received(value));
    let mut fields = vec![("expected", Json::String(expected))];
    if let Some(format) = format {
        fields.push(("format", Json::str(format)));
    }
    fields.extend([
        ("code", Json::str("invalid_type")),
        ("path", Json::Array(path.to_vec())),
        ("message", Json::String(message)),
    ]);
    object(fields)
}

fn invalid_value(values: &[Json], path: &[Json]) -> Json {
    let message = match values {
        [one] => format!("Invalid input: expected {}", issue_value(one)),
        _ => format!("Invalid option: expected one of {}", issue_values(values, "|")),
    };
    object(vec![
        ("code", Json::str("invalid_value")),
        ("values", Json::Array(values.to_vec())),
        ("path", Json::Array(path.to_vec())),
        ("message", Json::String(message)),
    ])
}

fn size(code: &str, origin: &str, limit: &Json, path: &[Json]) -> Json {
    let (label, comparison, key) = if code == "too_big" { ("Too big", "<=", "maximum") } else { ("Too small", ">=", "minimum") };
    let limit_text = js_string(limit);
    let message = match origin {
        "string" => format!("{label}: expected string to have {comparison}{limit_text} characters"),
        "array" => format!("{label}: expected array to have {comparison}{limit_text} items"),
        _ => format!("{label}: expected {origin} to be {comparison}{limit_text}"),
    };
    object(vec![
        ("origin", Json::str(origin)),
        ("code", Json::str(code)),
        (key, limit.clone()),
        ("inclusive", Json::Bool(true)),
        ("path", Json::Array(path.to_vec())),
        ("message", Json::String(message)),
    ])
}

fn format_error(tool: &str, issues: &[Json]) -> String {
    let code = |issue: &Json| issue.get("code").and_then(Json::as_str).unwrap_or_default().to_owned();
    let message = |issue: &Json| issue.get("message").and_then(Json::as_str).unwrap_or_default().to_owned();
    let path = |issue: &Json| format_path(issue.get("path").and_then(Json::as_array).unwrap_or_default());
    let is_missing = |issue: &Json| code(issue) == "invalid_type" && message(issue).contains("received undefined");
    let mut lines: Vec<String> = issues
        .iter()
        .filter(|i| is_missing(i))
        .map(|i| format!("The required parameter `{}` is missing", path(i)))
        .collect();
    for issue in issues.iter().filter(|i| code(i) == "unrecognized_keys") {
        for key in issue.get("keys").and_then(Json::as_array).unwrap_or_default() {
            lines.push(format!("An unexpected parameter `{}` was provided", key.as_str().unwrap_or_default()));
        }
    }
    for issue in issues.iter().filter(|i| code(i) == "invalid_type" && !is_missing(i)) {
        let text = message(issue);
        let received = text
            .split_once("received ")
            .and_then(|(_, rest)| {
                let word: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                (!word.is_empty()).then_some(word)
            })
            .unwrap_or_else(|| "unknown".into());
        let expected = issue.get("expected").and_then(Json::as_str).unwrap_or_default();
        lines.push(format!("The parameter `{}` type is expected as `{expected}` but provided as `{received}`", path(issue)));
    }
    if lines.is_empty() {
        return Json::Array(issues.to_vec()).pretty();
    }
    let noun = if lines.len() > 1 { "issues" } else { "issue" };
    format!("{tool} failed due to the following {noun}:\n{}", lines.join("\n"))
}

fn format_path(path: &[Json]) -> String {
    let mut out = String::new();
    for (index, segment) in path.iter().enumerate() {
        match segment {
            Json::Number(n) => out.push_str(&format!("[{n}]")),
            other if index == 0 => out.push_str(other.as_str().unwrap_or_default()),
            other => {
                out.push('.');
                out.push_str(other.as_str().unwrap_or_default());
            }
        }
    }
    out
}

fn received(value: Option<&Json>) -> &'static str {
    match value {
        None => "undefined",
        Some(Json::Null) => "null",
        Some(Json::Bool(_)) => "boolean",
        Some(Json::Number(_)) => "number",
        Some(Json::String(_)) => "string",
        Some(Json::Array(_)) => "array",
        Some(Json::Object(_)) => "object",
    }
}

/// JS `Object.is` 在 JSON 值上的语义：对象与数组按引用比较，因此永不相等。
fn same(value: Option<&Json>, expected: &Json) -> bool {
    match (value, expected) {
        (Some(Json::Null), Json::Null) => true,
        (Some(Json::Bool(a)), Json::Bool(b)) => a == b,
        (Some(Json::Number(a)), Json::Number(b)) => a.as_f64() == b.as_f64(),
        (Some(Json::String(a)), Json::String(b)) => a == b,
        _ => false,
    }
}

/// JS `String(value)`。
fn js_string(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        Json::Array(items) => items.iter().map(|i| if matches!(i, Json::Null) { String::new() } else { js_string(i) }).collect::<Vec<_>>().join(","),
        Json::Object(_) => "[object Object]".into(),
        other => other.compact(),
    }
}

/// TS `formatIssueValue`：字符串加双引号，其余 `String(value)`。
fn issue_value(value: &Json) -> String {
    match value {
        Json::String(text) => format!("\"{text}\""),
        other => js_string(other),
    }
}

fn issue_values(values: &[Json], separator: &str) -> String {
    values.iter().map(issue_value).collect::<Vec<_>>().join(separator)
}

fn number(value: Option<&Json>) -> Option<f64> {
    match value {
        Some(Json::Number(n)) => n.as_f64(),
        _ => None,
    }
}

fn schema_array(value: Option<&Json>) -> Option<&[Json]> {
    let items = value?.as_array()?;
    items.iter().all(Json::is_object).then_some(items)
}

fn child(path: &[Json], segment: Json) -> Path {
    let mut next = path.to_vec();
    next.push(segment);
    next
}

fn object(fields: Vec<(&str, Json)>) -> Json {
    Json::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

#[cfg(test)]
#[path = "tool_input_validation_tests.rs"]
mod tests;
