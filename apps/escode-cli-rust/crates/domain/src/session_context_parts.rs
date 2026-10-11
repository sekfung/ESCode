//! TS `session-context/parts.ts` 与 `activeSessionMessages`（无 rewind 选项时的 compaction 边界与保留段）。
//! 消息为 TS `MessageWithParts` 的 JSON 形态；工具 part 可带 `inputJson`（保持原键顺序的 input 文本）。

use serde_json::Value;

use crate::session_context_text::{stringify_in_order, truncate_text};

const TOOL_INPUT_PREVIEW_CHARS: usize = 900;
const TOOL_OUTPUT_PREVIEW_CHARS: usize = 1600;
const TEXT_PART_PREVIEW_CHARS: usize = 3000;
const FILE_PREVIEW_CHARS: usize = 1600;
const SKIPPED_SYNTHETIC_SOURCES: [&str; 9] = [
    "background_task",
    "subagent_message",
    "diagnostics",
    "goal_state_change",
    "hook_context",
    "model_anomaly",
    "queued_system_notification",
    "runtime_mode",
    "todo_reminder",
];

/// 把 TS JSON 文本解析为消息，并为工具 part 补上保持原键顺序的 `inputJson`。
pub fn message_from_ts_json(raw: &str) -> serde_json::Result<Value> {
    #[derive(serde::Deserialize)]
    struct RawMessage<'a> {
        #[serde(borrow, default)]
        parts: Vec<RawPart<'a>>,
    }
    #[derive(serde::Deserialize)]
    struct RawPart<'a> {
        #[serde(borrow, default)]
        state: Option<RawState<'a>>,
    }
    #[derive(serde::Deserialize)]
    struct RawState<'a> {
        #[serde(borrow, default)]
        input: Option<&'a serde_json::value::RawValue>,
    }
    let mut message: Value = serde_json::from_str(raw)?;
    let order: RawMessage = serde_json::from_str(raw)?;
    if let Some(parts) = message["parts"].as_array_mut() {
        for (part, raw_part) in parts.iter_mut().zip(order.parts) {
            if let Some(input) = raw_part.state.and_then(|s| s.input)
                && let Some(text) = stringify_in_order(input.get())
            {
                part["inputJson"] = text.into();
            }
        }
    }
    Ok(message)
}

/// TS `formatPartForContext`。
pub fn format_part(part: &Value) -> Option<String> {
    let text = |key: &str| part[key].as_str().unwrap_or_default();
    match part["type"].as_str()? {
        "text" => {
            let value = text("text");
            if part["ignored"] == true || contains_reminder_tag(value) {
                return None;
            }
            if part["synthetic"] == true
                && part["metadata"]["source"]
                    .as_str()
                    .is_some_and(|s| SKIPPED_SYNTHETIC_SOURCES.contains(&s))
            {
                return None;
            }
            Some(truncate_text(value, TEXT_PART_PREVIEW_CHARS))
        }
        "file" => Some(format_file(part)),
        "agent" => Some(format!("[Selected agent: {}]", text("name"))),
        "subtask" => {
            let mut lines = vec![format!("[Subtask: {}]", text("description"))];
            if !text("command").is_empty() {
                lines.push(format!("command: {}", text("command")));
            }
            lines.push(format!(
                "prompt: {}",
                truncate_text(text("prompt"), TEXT_PART_PREVIEW_CHARS)
            ));
            Some(lines.join("\n"))
        }
        "tool" => Some(format_tool(part)),
        "patch" => {
            let files: Vec<&str> = part["files"]
                .as_array()
                .map(|f| f.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            Some(format!("Patch files: {}", files.join(", ")))
        }
        "compaction" => part["timelineText"]
            .as_str()
            .or_else(|| part["reason"].as_str())
            .map(str::to_owned),
        "retry" => Some(format!(
            "Retry {}: {}",
            js_value(&part["attempt"]),
            part["error"]["name"].as_str().unwrap_or("undefined")
        )),
        "step-finish" => Some(format!("Step finished: {}", js_value(&part["reason"]))),
        _ => None,
    }
}

/// JS 模板字符串里的值：字符串原样，其余按 JSON。
fn js_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

/// TS：`/<\/?system-reminder\b/i`。
fn contains_reminder_tag(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["<system-reminder", "</system-reminder"].iter().any(|tag| {
        lower.match_indices(tag).any(|(index, _)| {
            lower[index + tag.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
        })
    })
}

fn format_file(part: &Value) -> String {
    let source = &part["source"];
    let path = match source["type"].as_str() {
        Some("resource") => source["uri"].as_str(),
        Some(_) => source["path"].as_str(),
        None => None,
    };
    let preview = part["metadata"]["preview"]["text"]
        .as_str()
        .or_else(|| source["text"]["value"].as_str());
    let mut header = vec!["File attachment".to_owned()];
    if let Some(name) = part["filename"].as_str().filter(|n| !n.is_empty()) {
        header.push(format!("filename={name}"));
    }
    header.push(format!("mime={}", js_value(&part["mime"])));
    if let Some(path) = path.filter(|p| !p.is_empty()) {
        header.push(format!("path={path}"));
    }
    let header = header.join(" ");
    match preview.filter(|p| !p.is_empty()) {
        Some(preview) => format!("{header}\n{}", truncate_text(preview, FILE_PREVIEW_CHARS)),
        None => header,
    }
}

fn format_tool(part: &Value) -> String {
    let state = &part["state"];
    let status = state["status"].as_str().unwrap_or_default();
    let mut lines = vec![format!("Tool {} {status}", js_value(&part["tool"]))];
    if state.get("input").is_some() {
        let input = part["inputJson"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| state["input"].to_string());
        lines.push(format!(
            "input: {}",
            truncate_text(&input, TOOL_INPUT_PREVIEW_CHARS)
        ));
    }
    let field = |key: &str| state[key].as_str().unwrap_or_default();
    match status {
        "completed" => lines.push(format!(
            "output: {}",
            truncate_text(field("output"), TOOL_OUTPUT_PREVIEW_CHARS)
        )),
        "error" => lines.push(format!(
            "error: {}",
            truncate_text(field("error"), TOOL_OUTPUT_PREVIEW_CHARS)
        )),
        "pending" => lines.push(format!(
            "raw: {}",
            truncate_text(field("raw"), TOOL_INPUT_PREVIEW_CHARS)
        )),
        _ => {}
    }
    lines.join("\n")
}

/// TS `dedupeParts`：按 id 去重，保留首次位置、最后一次的值。
pub fn dedupe_parts(parts: &[Value]) -> Vec<&Value> {
    let mut out: Vec<&Value> = Vec::new();
    for part in parts {
        match out.iter_mut().find(|existing| existing["id"] == part["id"]) {
            Some(slot) => *slot = part,
            None => out.push(part),
        }
    }
    out
}

/// TS `activeSessionMessages(messages)`（无 rewind 选项）：最后一个活跃 compaction 边界起，插入保留段。
pub fn active_messages(messages: &[Value]) -> Vec<Value> {
    let Some(boundary) = messages
        .iter()
        .rposition(|m| parts(m).iter().any(is_boundary))
    else {
        return messages.to_vec();
    };
    let active = &messages[boundary..];
    let segment = parts(&messages[boundary])
        .iter()
        .find(|p| p["type"] == "compaction" && p["compactBoundary"].is_object())
        .map(|p| &p["compactBoundary"]["preservedSegment"]);
    let Some(segment) = segment.filter(|s| s.is_object()) else {
        return active.to_vec();
    };
    let position = |key: &str| {
        messages
            .iter()
            .position(|m| m["info"]["id"] == segment[key] && !segment[key].is_null())
    };
    let preserved: Vec<Value> = match (position("headMessageId"), position("tailMessageId")) {
        (Some(head), Some(tail)) if tail >= head && tail < boundary => messages[head..=tail]
            .iter()
            .filter(|m| preservable(m))
            .cloned()
            .collect(),
        _ => vec![],
    };
    if preserved.is_empty() {
        return active.to_vec();
    }
    let insert = active
        .iter()
        .position(|m| m["info"]["id"] == segment["anchorMessageId"])
        .map_or(1, |i| i + 1)
        .min(active.len());
    let mut result = active[..insert].to_vec();
    result.extend(preserved);
    result.extend_from_slice(&active[insert..]);
    result
}

fn parts(message: &Value) -> &[Value] {
    message["parts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn is_boundary(part: &Value) -> bool {
    part["type"] == "compaction"
        && (part["compactBoundary"].is_object() || !truthy(&part["timelineStatus"]))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        _ => true,
    }
}

fn preservable(message: &Value) -> bool {
    let info = &message["info"];
    if info["semantics"]["providerVisibility"] == "hidden"
        || parts(message).iter().any(|p| p["type"] == "compaction")
        || (info["role"] == "assistant" && truthy(&info["error"]))
    {
        return false;
    }
    matches!(info["role"].as_str(), Some("user" | "assistant"))
}
