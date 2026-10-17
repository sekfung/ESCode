//! Claude Code jsonl → 可见消息（TS `claudeNativeSessionImportParser.ts` 的导入子集）。
//!
//! 只保留导入必需语义：可见 user/assistant 正文、连续 assistant 聚合、时间戳与模型；
//! 未知命令 warning / 可见 assistant turn 选择属于后续 repair 流程（spec §15.2 登记差异）。

use serde_json::{Map, Value, json};
use std::path::Path;

use super::head::{derive_title, sanitize_visible_text};
use super::timestamp::to_timestamp_ms;

#[derive(Debug, Clone)]
pub struct ImportedSession {
    pub session_id: String,
    pub workspace_path: String,
    pub source_path: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub title: String,
    pub model: Option<String>,
    pub messages: Vec<Value>,
}

pub(super) fn read_json_lines(path: &Path) -> Result<Vec<Value>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut records = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str(line)
            .map_err(|error| format!("解析 JSONL 失败 {}:{} {error}", path.display(), index + 1))?;
        records.push(value);
    }
    Ok(records)
}

pub(super) fn read_head(path: &Path, max_lines: usize) -> Result<Vec<Value>, String> {
    if max_lines == 0 {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut records = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str(line)
            .map_err(|error| format!("解析 JSONL 失败 {}:{} {error}", path.display(), index + 1))?;
        records.push(value);
        if records.len() >= max_lines {
            break;
        }
    }
    Ok(records)
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

pub(super) fn text_field(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

pub(super) fn read_workspace_path(entry: &Value) -> Option<String> {
    let message = object(entry).and_then(|record| record.get("message"));
    let request = object(entry).and_then(|record| record.get("request"));
    text_field(&entry["cwd"])
        .or_else(|| message.and_then(|message| text_field(&message["cwd"])))
        .or_else(|| request.and_then(|request| text_field(&request["cwd"])))
        .map(str::to_owned)
}

pub(super) fn read_entry_model(entry: &Value) -> Option<String> {
    let message = object(entry).and_then(|record| record.get("message"));
    text_field(&entry["model"])
        .or_else(|| message.and_then(|message| text_field(&message["model"])))
        .map(str::to_owned)
}

pub(super) fn read_entry_timestamp(entry: &Value) -> Option<i64> {
    let message = object(entry).and_then(|record| record.get("message"));
    let request = object(entry).and_then(|record| record.get("request"));
    let candidates = [
        entry.get("timestamp"),
        entry.get("createdAt"),
        entry.get("updatedAt"),
        entry.get("created_at"),
        entry.get("updated_at"),
        entry.get("time"),
        message.and_then(|message| message.get("timestamp")),
        message.and_then(|message| message.get("createdAt")),
        message.and_then(|message| message.get("updatedAt")),
        request.and_then(|request| request.get("timestamp")),
    ];
    candidates.into_iter().flatten().find_map(to_timestamp_ms)
}

fn extract_text_field(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_object()
        .and_then(|record| text_field(&record["text"]).or_else(|| text_field(&record["content"])))
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn extract_user_text(entry: &Value) -> Option<String> {
    if entry["type"] != "user" {
        return None;
    }
    let message = object(entry).and_then(|record| record.get("message"));
    if entry["isMeta"] == true || message.is_some_and(|message| message["isMeta"] == true) {
        return None;
    }
    let content = message
        .and_then(|message| message.get("content"))
        .or_else(|| object(entry).and_then(|record| record["request"].get("prompt")))?;
    if let Some(text) = content.as_str() {
        let normalized = sanitize_visible_text(text);
        return (!normalized.is_empty()).then_some(normalized);
    }
    let parts: Vec<String> = content
        .as_array()?
        .iter()
        .flat_map(|item| {
            if let Some(text) = item.as_str() {
                return vec![sanitize_visible_text(text)];
            }
            if item["type"] == "tool_result" {
                return Vec::new();
            }
            vec![sanitize_visible_text(&extract_text_field(item))]
        })
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub(super) fn extract_assistant_text(entry: &Value) -> Option<String> {
    if entry["type"] != "assistant" {
        return None;
    }
    let message = object(entry).and_then(|record| record.get("message"));
    let non_visible = entry["isApiErrorMessage"] == true
        || message.is_some_and(|message| message["isApiErrorMessage"] == true)
        || read_entry_model(entry).as_deref() == Some("<synthetic>");
    if non_visible {
        return None;
    }
    let content = message.and_then(|message| message.get("content"))?;
    if let Some(text) = content.as_str() {
        let normalized = sanitize_visible_text(text);
        return (!normalized.is_empty()).then_some(normalized);
    }
    let text: String = content
        .as_array()?
        .iter()
        .flat_map(|item| {
            if let Some(text) = item.as_str() {
                return vec![sanitize_visible_text(text)];
            }
            if item["type"] != "text" {
                return Vec::new();
            }
            vec![sanitize_visible_text(&extract_text_field(item))]
        })
        .filter(|part| !part.is_empty())
        .collect();
    (!text.is_empty()).then_some(text)
}

pub(super) fn parse_session_file(
    file_path: &Path,
    workspace_path: &str,
    session_id: &str,
    source_path: &str,
    fallback_created_at: Option<i64>,
    fallback_updated_at: Option<i64>,
) -> Result<ImportedSession, String> {
    let stat = std::fs::metadata(file_path)
        .map_err(|error| format!("stat {}: {error}", file_path.display()))?;
    let created_fallback = fallback_created_at.unwrap_or_else(|| mtime_ms(&stat));
    let updated_fallback = fallback_updated_at.unwrap_or_else(|| mtime_ms(&stat));
    let records = read_json_lines(file_path)?;
    parse_records(
        &records,
        workspace_path,
        session_id,
        source_path,
        Some(created_fallback),
        Some(updated_fallback),
    )
}

fn mtime_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(super) fn parse_records(
    records: &[Value],
    workspace_path: &str,
    session_id: &str,
    source_path: &str,
    fallback_created_at: Option<i64>,
    fallback_updated_at: Option<i64>,
) -> Result<ImportedSession, String> {
    let mut messages = Vec::new();
    let mut detected_workspace = None;
    let mut first_user_timestamp = None;
    let mut updated_at = fallback_updated_at;
    let mut current_turn = -1_i64;
    let mut model: Option<String> = None;
    let mut pending_content = String::new();
    let mut pending_timestamp: Option<i64> = None;

    let flush = |messages: &mut Vec<Value>,
                 content: &mut String,
                 timestamp: &mut Option<i64>,
                 model: &Option<String>,
                 updated_at: &mut Option<i64>,
                 turn: i64| {
        if content.is_empty() || turn < 0 {
            content.clear();
            *timestamp = None;
            return;
        }
        let mut message = json!({
            "role": "assistant",
            "content": content,
            "timestamp": timestamp.or(*updated_at).unwrap_or(0),
            "turnIndex": turn,
        });
        if let Some(model) = model {
            message["model"] = Value::from(model.clone());
        }
        messages.push(message);
        *updated_at = timestamp.or(*updated_at);
        content.clear();
        *timestamp = None;
    };

    for entry in records {
        if let Some(user) = extract_user_text(entry) {
            if detected_workspace.is_none() {
                detected_workspace = read_workspace_path(entry);
            }
            flush(
                &mut messages,
                &mut pending_content,
                &mut pending_timestamp,
                &model,
                &mut updated_at,
                current_turn,
            );
            current_turn += 1;
            let timestamp = read_entry_timestamp(entry).or(updated_at).unwrap_or(0);
            first_user_timestamp.get_or_insert(timestamp);
            updated_at = Some(timestamp);
            messages.push(json!({
                "role": "user",
                "content": user,
                "timestamp": timestamp,
                "turnIndex": current_turn,
            }));
            continue;
        }
        let Some(assistant) = extract_assistant_text(entry) else {
            continue;
        };
        if detected_workspace.is_none() {
            detected_workspace = read_workspace_path(entry);
        }
        if current_turn < 0 {
            continue;
        }
        if model.is_none() {
            model = read_entry_model(entry);
        }
        pending_content.push_str(&assistant);
        pending_timestamp = read_entry_timestamp(entry)
            .or(pending_timestamp)
            .or(updated_at);
        updated_at = pending_timestamp.or(updated_at);
    }
    flush(
        &mut messages,
        &mut pending_content,
        &mut pending_timestamp,
        &model,
        &mut updated_at,
        current_turn,
    );

    if messages.is_empty() {
        return Err(format!(
            "[claude-native] Claude 原生 session {session_id} 没有可导入的可见消息"
        ));
    }
    let created_at = first_user_timestamp
        .or(fallback_created_at)
        .or(updated_at)
        .unwrap_or(0);
    let updated_at = updated_at
        .or(first_user_timestamp)
        .or(fallback_created_at)
        .unwrap_or(created_at);
    let title = messages
        .iter()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .map(derive_title)
        .unwrap_or_default();
    Ok(ImportedSession {
        session_id: session_id.to_owned(),
        workspace_path: detected_workspace.unwrap_or_else(|| workspace_path.to_owned()),
        source_path: source_path.to_owned(),
        created_at,
        updated_at,
        title,
        model,
        messages,
    })
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
