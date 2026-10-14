//! 工具入参校验的 issue 构造（从 tool_input_validation.rs 拆出）。

use super::tool_input_validation::{issue_value, issue_values, js_string, node, object, received};
use crate::json_order::Json;

/// TS `createMissingPropertyIssues`：先按属性 schema 校验 undefined，没有问题时回落 invalid_type。
pub(super) fn missing(property: &Json, path: &[Json], issues: &mut Vec<Json>) {
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
            _ if matches!(property.get("properties"), Some(Json::Object(_)))
                || property.get("required").and_then(Json::as_array).is_some() =>
            {
                "object".into()
            }
            _ if matches!(property.get("items"), Some(Json::Object(_))) => "array".into(),
            _ => "unknown".into(),
        },
        _ => "unknown".into(),
    };
    issues.push(invalid_type(None, &Json::String(inferred), path));
}

pub(super) fn matches_type(value: Option<&Json>, kind: &Json) -> bool {
    match kind {
        Json::Array(kinds) => kinds.iter().any(|k| matches_type(value, k)),
        Json::String(kind) => match kind.as_str() {
            "array" => matches!(value, Some(Json::Array(_))),
            "boolean" => matches!(value, Some(Json::Bool(_))),
            "integer" => {
                matches!(value, Some(Json::Number(n)) if n.as_f64().is_some_and(|f| f.fract() == 0.0))
            }
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
pub(super) fn invalid_type(value: Option<&Json>, kind: &Json, path: &[Json]) -> Json {
    let kind = js_string(kind);
    let (expected, format) = match (kind.as_str(), value) {
        ("integer", Some(Json::Number(_))) => ("int".to_owned(), Some("safeint")),
        ("integer", _) => ("number".to_owned(), None),
        _ => (kind, None),
    };
    let message = format!(
        "Invalid input: expected {expected}, received {}",
        received(value)
    );
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

pub(super) fn invalid_value(values: &[Json], path: &[Json]) -> Json {
    let message = match values {
        [one] => format!("Invalid input: expected {}", issue_value(one)),
        _ => format!(
            "Invalid option: expected one of {}",
            issue_values(values, "|")
        ),
    };
    object(vec![
        ("code", Json::str("invalid_value")),
        ("values", Json::Array(values.to_vec())),
        ("path", Json::Array(path.to_vec())),
        ("message", Json::String(message)),
    ])
}

pub(super) fn size(code: &str, origin: &str, limit: &Json, path: &[Json]) -> Json {
    let (label, comparison, key) = if code == "too_big" {
        ("Too big", "<=", "maximum")
    } else {
        ("Too small", ">=", "minimum")
    };
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
