//! model-IO 记录的请求 / 响应投影（TS model-io 的 message / usage / tool call 形状）。
#[allow(unused_imports)]
use super::model_io::*;

use crate::contract::ModelOutput;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// 原始请求体 → JSON。`keep_messages` 为假时 `messages` 只按原始片段跳过，不构建值树。
pub(super) fn request_body(body: &[u8], keep_messages: bool) -> Option<Value> {
    if keep_messages {
        return serde_json::from_slice(body).ok();
    }
    let fields: std::collections::BTreeMap<String, &serde_json::value::RawValue> =
        serde_json::from_slice(body).ok()?;
    let mut out = Map::new();
    for (key, raw) in fields {
        if key != "messages" {
            out.insert(key, serde_json::from_str(raw.get()).ok()?);
        }
    }
    Some(Value::Object(out))
}

pub(super) fn response(output: &ModelOutput) -> Value {
    let message = &output.message;
    let mut response = Map::new();
    let finish = if output.output_limit {
        "length"
    } else if output.calls.is_empty() {
        "stop"
    } else {
        "tool-calls"
    };
    response.insert("finishReason".into(), finish.into());
    if !output.response_id.is_empty() {
        response.insert("responseId".into(), output.response_id.clone().into());
    }
    response.insert(
        "text".into(),
        message["content"].as_str().unwrap_or_default().into(),
    );
    if let Some(reasoning) = message["reasoning_content"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
    {
        response.insert("reasoningText".into(), reasoning.into());
    }
    response.insert(
        "toolCalls".into(),
        Value::Array(output.calls.iter().map(tool_call).collect()),
    );
    response.insert("usage".into(), usage(&output.usage));
    Value::Object(response)
}

/// TS `normalizeUsage` 的字段（Rust 各协议的 usage 已统一成 OpenAI chat 形）。
pub(super) fn usage(raw: &Value) -> Value {
    let mut usage = Map::new();
    let input = raw["prompt_tokens"].as_u64();
    let output = raw["completion_tokens"].as_u64();
    for (key, value) in [
        ("inputTokens", input),
        ("outputTokens", output),
        (
            "totalTokens",
            raw["total_tokens"].as_u64().or(match (input, output) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            }),
        ),
        // AI SDK（openai-compatible）在有 usage 时把缺失的 cached / reasoning 计为 0；cacheWrite 只在上报时出现。
        (
            "cacheReadTokens",
            raw["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .or(input.map(|_| 0)),
        ),
        (
            "cacheWriteTokens",
            raw["prompt_tokens_details"]["cache_write_tokens"].as_u64(),
        ),
        (
            "reasoningTokens",
            raw["completion_tokens_details"]["reasoning_tokens"]
                .as_u64()
                .or(output.map(|_| 0)),
        ),
    ] {
        if let Some(value) = value {
            usage.insert(key.into(), value.into());
        }
    }
    Value::Object(usage)
}

/// OpenAI chat 形工具调用 → TS 归一化的 `{id, name, input}`。
pub(super) fn tool_call(call: &Value) -> Value {
    let arguments = &call["function"]["arguments"];
    let input = match arguments.as_str() {
        Some(text) => serde_json::from_str(text).unwrap_or_else(|_| Value::from(text)),
        None => arguments.clone(),
    };
    json!({ "id": call["id"], "name": call["function"]["name"], "input": input })
}

/// 投影整段历史。tool 消息的 `toolName`：持久化后的历史只在带媒体时保留 `_escode_tool_name`，
/// 因此按 `tool_call_id` 回查前面 assistant 的 `tool_calls`（与 TS 结果消息上的工具名同源）。
pub(super) fn project_messages(messages: &[Value]) -> Vec<Value> {
    let mut names: HashMap<String, String> = HashMap::new();
    messages
        .iter()
        .map(|message| {
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                if let (Some(id), Some(name)) =
                    (call["id"].as_str(), call["function"]["name"].as_str())
                {
                    names.insert(id.to_owned(), name.to_owned());
                }
            }
            let mut projected = project_message(message);
            if projected["role"] == "tool"
                && projected.get("toolName").is_none()
                && let Some(name) = projected["toolCallId"]
                    .as_str()
                    .and_then(|id| names.get(id))
            {
                projected["toolName"] = name.as_str().into();
            }
            projected
        })
        .collect()
}

/// Rust 内部历史（OpenAI chat 形 + `_escode_*` 私有字段）→ TS `ModelInputMessage` 形。
pub(super) fn project_message(message: &Value) -> Value {
    let role = message["role"].as_str().unwrap_or("user");
    let mut out = Map::new();
    out.insert("role".into(), role.into());
    match role {
        "assistant" => {
            let text = message["content"].as_str().unwrap_or_default();
            let reasoning = message["reasoning_content"]
                .as_str()
                .filter(|text| !text.is_empty());
            let content = match reasoning {
                Some(reasoning) => {
                    let mut parts = vec![json!({ "type": "reasoning", "text": reasoning })];
                    if !text.is_empty() {
                        parts.push(json!({ "type": "text", "text": text }));
                    }
                    Value::Array(parts)
                }
                None => project_content(&message["content"]),
            };
            out.insert("content".into(), content);
            if let Some(calls) = message["tool_calls"].as_array().filter(|c| !c.is_empty()) {
                out.insert(
                    "toolCalls".into(),
                    Value::Array(calls.iter().map(tool_call).collect()),
                );
            }
        }
        "tool" => {
            out.insert("content".into(), project_content(&message["content"]));
            if let Some(id) = message["tool_call_id"].as_str() {
                out.insert("toolCallId".into(), id.into());
            }
            if let Some(name) = message["_escode_tool_name"].as_str() {
                out.insert("toolName".into(), name.into());
            }
            if message["_escode_tool_failed"] == true {
                out.insert("isError".into(), true.into());
            }
        }
        _ => {
            out.insert("content".into(), project_content(&message["content"]));
        }
    }
    Value::Object(out)
}

pub(super) fn project_content(content: &Value) -> Value {
    match content {
        Value::Array(parts) => Value::Array(
            parts
                .iter()
                .map(|part| match part["type"].as_str() {
                    Some("text") => json!({ "type": "text", "text": part["text"] }),
                    Some("image_url") => {
                        let url = part["image_url"]["url"].as_str().unwrap_or_default();
                        let media = url
                            .strip_prefix("data:")
                            .and_then(|rest| rest.split(';').next())
                            .unwrap_or("image");
                        json!({ "type": "image", "mediaType": media })
                    }
                    _ => {
                        let mut part = part.clone();
                        if let Some(object) = part.as_object_mut() {
                            object.retain(|key, _| !key.starts_with("_escode"));
                        }
                        part
                    }
                })
                .collect(),
        ),
        Value::Null => Value::from(""),
        other => other.clone(),
    }
}

pub(super) fn millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// JS `Date#toISOString`。
pub(super) fn iso(time: SystemTime) -> String {
    let millis = millis(time) as i64;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

// ---- 写入：淘汰 / 上限 / 生产裁剪 / delta 压缩（TS writeModelIODebugRecord） ----
