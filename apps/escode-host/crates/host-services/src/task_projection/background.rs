//! 后台任务通知投影（`packages/shared/src/background-task-notifications.ts`）。
//!
//! 通知文本解析、按 tool-use-id 收集、写进 tool raw 的 `_meta.escode.taskNotification`、
//! 通知状态 → tool 更新状态。控制项解析（`background-task-controls.ts`）在 `background_controls.rs`。

use std::collections::BTreeMap;

use serde_json::{Map, Value};

pub fn collect_notifications_by_tool_use_id(messages: &[Value]) -> BTreeMap<String, Value> {
    let mut notifications = BTreeMap::new();
    for message in messages {
        let info = field(message, "info").unwrap_or(&Value::Null);
        if field(info, "role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let parts = array_field(message, "parts");
        let parsed = parse_notification_text(&parts_text(parts));
        if let Some((tool_use_id, notification)) = parsed {
            notifications.insert(tool_use_id, notification);
        }
    }
    notifications
}

/// TS `escodeBackgroundTaskNotificationToolUpdateStatus`。
pub fn notification_tool_update_status(status: &str) -> Option<&'static str> {
    match status {
        "failed" | "lost" => Some("failed"),
        // task-notification 的 killed/stopped 都表示被停止，不能折成 completed。
        "stopped" | "killed" => Some("stopped"),
        _ => Some("completed"),
    }
}

/// TS `attachESCodeBackgroundTaskNotificationToRaw`：通知挂到 `_meta.escode.taskNotification`。
pub fn attach_notification_to_raw(raw: Value, notification: Option<&Value>) -> Value {
    let Some(notification) = notification else {
        return raw;
    };
    let mut record = plain_record(raw);
    let mut meta = plain_record(record.remove("_meta").unwrap_or(Value::Null));
    let mut escode = plain_record(meta.remove("escode").unwrap_or(Value::Null));
    escode.insert("taskNotification".to_owned(), notification.clone());
    meta.insert("escode".to_owned(), Value::Object(escode));
    record.insert("_meta".to_owned(), Value::Object(meta));
    Value::Object(record)
}

/// 控制项解析（`background-task-controls.ts`）在 `background_controls.rs`，这里再导出保持调用面单一。
fn parse_notification_text(text: &str) -> Option<(String, Value)> {
    let trimmed = text.trim();
    if !trimmed.starts_with("<task-notification>") {
        return None;
    }
    let tool_use_id = read_tag(trimmed, "tool-use-id")?;
    let mut notification = Map::new();
    for tag in [
        "error",
        "output-file",
        "result",
        "status",
        "summary",
        "task-id",
    ] {
        let key = match tag {
            "output-file" => "outputFile",
            "task-id" => "taskId",
            other => other,
        };
        if let Some(value) = read_tag(trimmed, tag) {
            notification.insert(key.to_owned(), Value::from(value));
        }
    }
    Some((tool_use_id, Value::Object(notification)))
}

/// TS `readTaskNotificationTag`：首个 `<tag>…</tag>`（非贪婪），trim 后解码 XML 实体。
fn read_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(&close)?;
    let raw = rest[..end].trim();
    (!raw.is_empty()).then(|| decode_xml_text(raw))
}
fn decode_xml_text(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
fn parts_text(parts: &[Value]) -> String {
    parts
        .iter()
        .filter(|part| part_type(part) == Some("text"))
        .map(|part| {
            field(part, "text")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect::<String>()
}
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}
fn array_field<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    field(value, key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
fn part_type(part: &Value) -> Option<&str> {
    field(part, "type").and_then(Value::as_str)
}

fn plain_record(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(record) => record,
        _ => Map::new(),
    }
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
