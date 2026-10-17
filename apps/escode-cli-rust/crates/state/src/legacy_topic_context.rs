use crate::domain::session::Session;
use serde_json::{Value, json};

/// TS 旧 `bot_topic_context`（docs/specs/rust-app-p0.md）：TS hydrator 恢复时跳过、不重新注入
/// 模型历史；这里同样不写 canonical 消息与行，只保留读路径需要的来源记录。
pub(super) fn project(
    session: &mut Session,
    message_id: &str,
    message: &Value,
    parts: &[(String, Value)],
) -> bool {
    if message["role"] != "user" || message["source"] != "bot_topic_context" {
        return false;
    }
    let text = parts
        .iter()
        .filter(|(_, p)| p["type"] == "text")
        .filter_map(|(_, p)| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let metadata = &message["metadata"];
    let mut record = json!({
        "messageId": message_id,
        "createdAt": message["time"]["created"],
        "rowAnchor": session.rows.len(),
        "text": text,
    });
    if let Some(checkpoint) = metadata.get("checkpoint").filter(|v| !v.is_null()) {
        record["checkpoint"] = checkpoint.clone();
    }
    if let Some(display) = metadata["displayText"].as_str() {
        record["displayText"] = display.into();
    }
    session.legacy_topic_records.push(record);
    true
}
