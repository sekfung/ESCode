//! `model_trajectory` 的记录映射（TS `mapRecord` 及其辅助）：从原始 model-io 记录到 UI 轨迹记录。

use serde_json::{Map, Value, json};

const TITLE_PROMPT_PREFIX: &str = "Generate a concise title for this coding session.";

pub(super) fn as_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn as_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn as_number(value: Option<&Value>) -> Option<Value> {
    value.filter(|v| v.is_number()).cloned()
}

fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value);
    }
}

fn string_value(value: Option<String>) -> Option<Value> {
    value.map(Value::String)
}

pub(super) fn classify_call_source(query_source: Option<&str>, role: Option<&str>) -> Value {
    match (query_source, role) {
        (Some("main_turn"), _) => json!({"kind": "main", "querySource": "main_turn"}),
        (Some("subagent"), _) => json!({"kind": "subagent", "querySource": "subagent"}),
        (Some(source), _) if source == "compact" => {
            json!({"kind": "compact", "querySource": source})
        }
        (source, Some("compact")) => with_source(json!({"kind": "compact"}), source),
        (Some(source), _) => json!({"kind": "sidecar", "querySource": source}),
        (None, Some("subagent")) => json!({"kind": "subagent"}),
        (None, _) => json!({"kind": "main"}),
    }
}

fn with_source(mut value: Value, source: Option<&str>) -> Value {
    if let Some(source) = source {
        value["querySource"] = Value::String(source.to_owned());
    }
    value
}

fn infer_query_source(request: Option<&Map<String, Value>>) -> Option<String> {
    let first = as_object(request?.get("messages")?.as_array()?.first())?;
    if first.get("role").and_then(Value::as_str) != Some("system") {
        return None;
    }
    let content = first.get("content").and_then(Value::as_str)?;
    content
        .starts_with(TITLE_PROMPT_PREFIX)
        .then(|| "session_title".to_owned())
}

fn try_parse_json(value: &str, is_error: bool) -> Value {
    if is_error {
        return json!({"type": "error-text", "value": value});
    }
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()))
}

#[derive(Default, Clone)]
struct MessageTool {
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    is_error: bool,
}

fn map_content(content: Option<&Value>, role: Option<&str>, tool: &MessageTool) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => {
            if text.is_empty() {
                return Vec::new();
            }
            if role == Some("tool") {
                let mut part = Map::new();
                part.insert("kind".into(), json!("tool-result"));
                put(
                    &mut part,
                    "toolCallId",
                    string_value(tool.tool_call_id.clone()),
                );
                put(&mut part, "toolName", string_value(tool.tool_name.clone()));
                part.insert("output".into(), try_parse_json(text, tool.is_error));
                return vec![Value::Object(part)];
            }
            vec![json!({"kind": "text", "text": text})]
        }
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| map_part(part, (role == Some("tool")).then_some(tool)))
            .collect(),
        _ => Vec::new(),
    }
}

fn map_part(raw: &Value, tool: Option<&MessageTool>) -> Value {
    let Some(part) = raw.as_object() else {
        return json!({"kind": "unknown", "raw": raw});
    };
    let get = |key: &str| as_string(part.get(key));
    match part.get("type").and_then(Value::as_str) {
        Some("text") => json!({"kind": "text", "text": get("text").unwrap_or_default()}),
        Some("reasoning") => json!({"kind": "reasoning", "text": get("text").unwrap_or_default()}),
        Some("tool-call") => {
            let mut out = Map::new();
            out.insert("kind".into(), json!("tool-call"));
            put(&mut out, "toolCallId", string_value(get("toolCallId")));
            out.insert(
                "toolName".into(),
                json!(get("toolName").unwrap_or_else(|| "tool".to_owned())),
            );
            put(
                &mut out,
                "input",
                part.get("input").or_else(|| part.get("args")).cloned(),
            );
            Value::Object(out)
        }
        Some("tool-result") => {
            let mut out = Map::new();
            out.insert("kind".into(), json!("tool-result"));
            put(
                &mut out,
                "toolCallId",
                string_value(
                    get("toolCallId").or_else(|| tool.and_then(|t| t.tool_call_id.clone())),
                ),
            );
            put(
                &mut out,
                "toolName",
                string_value(get("toolName").or_else(|| tool.and_then(|t| t.tool_name.clone()))),
            );
            put(
                &mut out,
                "output",
                part.get("output").or_else(|| part.get("result")).cloned(),
            );
            Value::Object(out)
        }
        Some("image" | "file") => {
            let mut out = Map::new();
            out.insert("kind".into(), json!("image"));
            put(&mut out, "mediaType", string_value(get("mediaType")));
            Value::Object(out)
        }
        _ => json!({"kind": "unknown", "raw": raw}),
    }
}

fn map_messages(value: Option<&Value>) -> Vec<Value> {
    let Some(list) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .map(|entry| {
            let empty = Map::new();
            let message = entry.as_object().unwrap_or(&empty);
            let role = as_string(message.get("role")).unwrap_or_else(|| "unknown".to_owned());
            let tool = MessageTool {
                tool_call_id: as_string(message.get("toolCallId"))
                    .or_else(|| as_string(message.get("tool_call_id")))
                    .or_else(|| as_string(message.get("tool_use_id"))),
                tool_name: as_string(message.get("toolName"))
                    .or_else(|| as_string(message.get("name"))),
                is_error: message.get("isError") == Some(&Value::Bool(true))
                    || message.get("is_error") == Some(&Value::Bool(true)),
            };
            json!({"role": role, "parts": map_content(message.get("content"), Some(&role), &tool)})
        })
        .collect()
}

fn map_response_tool_call(raw: &Value) -> Value {
    let Some(call) = raw.as_object() else {
        return json!({"kind": "unknown", "raw": raw});
    };
    let mut out = Map::new();
    out.insert("kind".into(), json!("tool-call"));
    put(
        &mut out,
        "toolCallId",
        string_value(as_string(call.get("id")).or_else(|| as_string(call.get("toolCallId")))),
    );
    out.insert(
        "toolName".into(),
        json!(
            as_string(call.get("name"))
                .or_else(|| as_string(call.get("toolName")))
                .unwrap_or_else(|| "tool".to_owned())
        ),
    );
    put(
        &mut out,
        "input",
        call.get("input").or_else(|| call.get("args")).cloned(),
    );
    Value::Object(out)
}

fn map_usage(value: Option<&Value>) -> Option<Value> {
    let usage = as_object(value)?;
    let mut out = Map::new();
    for key in [
        "inputTokens",
        "outputTokens",
        "totalTokens",
        "cacheReadTokens",
        "reasoningTokens",
    ] {
        put(&mut out, key, as_number(usage.get(key)));
    }
    Some(Value::Object(out))
}

pub(super) fn map_record(record: &Map<String, Value>) -> Value {
    let request = as_object(record.get("request"));
    let response = as_object(record.get("response"));
    let model = as_object(record.get("model"));
    let error = as_object(record.get("error"));
    let query_source = as_string(record.get("querySource")).or_else(|| infer_query_source(request));
    let role = model.and_then(|m| as_string(m.get("role")));
    let mut mapped = Map::new();
    mapped.insert(
        "requestId".into(),
        json!(as_string(record.get("requestId")).unwrap_or_default()),
    );
    mapped.insert(
        "attempt".into(),
        as_number(record.get("attempt")).unwrap_or_else(|| json!(1)),
    );
    mapped.insert(
        "startedAt".into(),
        json!(as_string(record.get("startedAt")).unwrap_or_default()),
    );
    put(
        &mut mapped,
        "completedAt",
        string_value(as_string(record.get("completedAt"))),
    );
    put(
        &mut mapped,
        "durationMs",
        as_number(record.get("durationMs")),
    );
    put(
        &mut mapped,
        "turnId",
        string_value(as_string(record.get("turnId"))),
    );
    put(
        &mut mapped,
        "traceId",
        string_value(as_string(record.get("traceId"))),
    );
    mapped.insert(
        "callSource".into(),
        classify_call_source(query_source.as_deref(), role.as_deref()),
    );
    let mut model_out = Map::new();
    put(
        &mut model_out,
        "modelId",
        string_value(model.and_then(|m| as_string(m.get("modelId")))),
    );
    put(
        &mut model_out,
        "providerId",
        string_value(model.and_then(|m| as_string(m.get("providerId")))),
    );
    put(&mut model_out, "role", string_value(role));
    put(
        &mut model_out,
        "source",
        string_value(model.and_then(|m| as_string(m.get("source")))),
    );
    mapped.insert("model".into(), Value::Object(model_out));
    let tool_names: Vec<Value> = request
        .and_then(|r| r.get("toolNames"))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter(|v| v.is_string()).cloned().collect())
        .unwrap_or_default();
    mapped.insert(
        "request".into(),
        json!({"messages": map_messages(request.and_then(|r| r.get("messages"))), "toolNames": tool_names}),
    );
    if let Some(response) = response {
        let calls: Vec<Value> = response
            .get("toolCalls")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(map_response_tool_call).collect())
            .unwrap_or_default();
        let mut out = Map::new();
        put(
            &mut out,
            "finishReason",
            string_value(as_string(response.get("finishReason"))),
        );
        put(
            &mut out,
            "text",
            string_value(as_string(response.get("text"))),
        );
        put(
            &mut out,
            "reasoningText",
            string_value(as_string(response.get("reasoningText"))),
        );
        out.insert("toolCalls".into(), Value::Array(calls));
        put(&mut out, "usage", map_usage(response.get("usage")));
        put(
            &mut out,
            "responseId",
            string_value(as_string(response.get("responseId"))),
        );
        put(
            &mut out,
            "modelId",
            string_value(as_string(response.get("modelId"))),
        );
        mapped.insert("response".into(), Value::Object(out));
    }
    if let Some(error) = error {
        let name = as_string(error.get("name"));
        let message = as_string(error.get("message"));
        let truthy = |v: &Option<String>| v.as_ref().is_some_and(|s| !s.is_empty());
        if truthy(&message) || truthy(&name) {
            let mut out = Map::new();
            out.insert(
                "name".into(),
                json!(name.unwrap_or_else(|| "Error".to_owned())),
            );
            out.insert("message".into(), json!(message.unwrap_or_default()));
            put(
                &mut out,
                "stack",
                string_value(as_string(error.get("stack"))),
            );
            mapped.insert("error".into(), Value::Object(out));
        }
    }
    Value::Object(mapped)
}
