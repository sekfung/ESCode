//! 把 Rust 会话（`rust_message` 聊天消息 + `rust_row` 行）转成 TS `MessageWithParts` 形态，
//! 供 ReadSessionContext 与 TS 会话走同一套素材逻辑。映射规则见 docs/specs/rust-read-session-context.md。

use serde_json::{Value, json};

use crate::session_context_text::stringify_in_order;

/// `messages` 为会话的聊天消息（全量历史）；`rows` 用于取时间与实体 id；
/// `offset` / `summary` 为 compaction 边界（offset 之前的消息已被摘要替代）。
pub fn messages_from_rust(
    messages: &[Value],
    rows: &[Value],
    offset: usize,
    summary: Option<&str>,
    fallback_time: i64,
) -> Vec<Value> {
    let mut clock = RowClock::new(rows, fallback_time);
    let mut out = Vec::new();
    if let Some(summary) = summary.filter(|_| offset > 0) {
        // TS 的 compaction 边界消息 + 摘要消息（活跃消息从边界起）。
        out.push(json!({
            "info": {"id": "compaction", "role": "user", "time": {"created": fallback_time}},
            "parts": [{"id": "compaction-p0", "type": "compaction", "reason": "context limit"}],
        }));
        out.push(json!({
            "info": {"id": "compaction-summary", "role": "assistant", "time": {"created": fallback_time}},
            "parts": [{"id": "compaction-summary-p0", "type": "text", "text": summary}],
        }));
    }
    let start = if summary.is_some() {
        offset.min(messages.len())
    } else {
        0
    };
    let mut index = start;
    while index < messages.len() {
        let message = &messages[index];
        index += 1;
        match message["role"].as_str() {
            Some("user") => out.push(user_message(message, &mut clock, index)),
            Some("assistant") => {
                // 紧随其后的 tool 结果归入同一 assistant 消息（TS 一步一条 assistant 消息）。
                let mut results = Vec::new();
                while index < messages.len() && messages[index]["role"] == "tool" {
                    results.push(&messages[index]);
                    index += 1;
                }
                out.push(assistant_message(message, &results, &mut clock, index));
            }
            _ => {}
        }
    }
    out
}

fn user_message(message: &Value, clock: &mut RowClock, ordinal: usize) -> Value {
    let reminder = message.get("_zcode_source").is_some()
        || message["content"]
            .as_str()
            .is_some_and(|c| c.starts_with("<system-reminder>"));
    let (id, created) = if reminder {
        (format!("msg_{ordinal}"), clock.fallback)
    } else {
        clock.next("userInput", ordinal)
    };
    let mut parts = Vec::new();
    match &message["content"] {
        Value::String(text) => parts.push(json!({"type": "text", "text": text})),
        Value::Array(items) => {
            for item in items {
                match item["type"].as_str() {
                    Some("text") => parts.push(json!({"type": "text", "text": item["text"]})),
                    Some("_zcode_attachment") => parts.push(json!({
                        "type": "file",
                        "mime": item["asset"]["mime"].as_str().unwrap_or("application/octet-stream"),
                        "filename": item["name"],
                    })),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let mut info = json!({"id": id, "role": "user", "time": {"created": created}});
    if reminder {
        info["visibility"] = "model-only".into();
    }
    with_part_ids(info, parts)
}

fn assistant_message(
    message: &Value,
    results: &[&Value],
    clock: &mut RowClock,
    ordinal: usize,
) -> Value {
    let calls = message["tool_calls"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let text = message["content"].as_str().unwrap_or_default();
    let (id, created) = if text.is_empty() {
        (format!("msg_{ordinal}"), clock.tool_time(calls))
    } else {
        clock.next("assistantText", ordinal)
    };
    let mut parts = vec![json!({"type": "step-start"})];
    // TS 在同一步里先落工具 part，文本 part 在步末写入。
    for call in calls {
        let raw = call["function"]["arguments"].as_str().unwrap_or("{}");
        let result = results.iter().find(|r| r["tool_call_id"] == call["id"]);
        let mut state = json!({"input": serde_json::from_str::<Value>(raw).unwrap_or(json!({}))});
        match result {
            Some(result) if result["_zcode_tool_failed"] == true => {
                state["status"] = "error".into();
                state["error"] = tool_text(&result["content"]);
            }
            Some(result) => {
                state["status"] = "completed".into();
                state["output"] = tool_text(&result["content"]);
            }
            None => state["status"] = "running".into(),
        }
        let mut part = json!({"type": "tool", "tool": call["function"]["name"], "callID": call["id"], "state": state});
        if let Some(input) = stringify_in_order(raw) {
            part["inputJson"] = input.into();
        }
        parts.push(part);
    }
    if !text.is_empty() {
        parts.push(json!({"type": "text", "text": text}));
    }
    let reason = if calls.is_empty() {
        "stop"
    } else {
        "tool-calls"
    };
    parts.push(json!({"type": "step-finish", "reason": reason}));
    with_part_ids(
        json!({"id": id, "role": "assistant", "time": {"created": created}}),
        parts,
    )
}

fn with_part_ids(info: Value, parts: Vec<Value>) -> Value {
    let id = info["id"].as_str().unwrap_or_default().to_owned();
    let parts: Vec<Value> = parts
        .into_iter()
        .enumerate()
        .map(|(index, mut part)| {
            part["id"] = format!("{id}-p{index}").into();
            part
        })
        .collect();
    json!({"info": info, "parts": parts})
}

/// 按行顺序为消息取实体 id 与创建时间；找不到对应行时退回会话时间。
struct RowClock<'a> {
    rows: &'a [Value],
    cursor: usize,
    fallback: i64,
}

impl<'a> RowClock<'a> {
    fn new(rows: &'a [Value], fallback: i64) -> Self {
        Self {
            rows,
            cursor: 0,
            fallback,
        }
    }

    fn next(&mut self, kind: &str, ordinal: usize) -> (String, i64) {
        match self.rows[self.cursor..]
            .iter()
            .position(|r| r["kind"] == kind)
        {
            Some(offset) => {
                let row = &self.rows[self.cursor + offset];
                self.cursor += offset + 1;
                (
                    row["entityId"]
                        .as_str()
                        .map_or_else(|| format!("msg_{ordinal}"), str::to_owned),
                    row["createdAt"].as_i64().unwrap_or(self.fallback),
                )
            }
            None => (format!("msg_{ordinal}"), self.fallback),
        }
    }

    fn tool_time(&self, calls: &[Value]) -> i64 {
        calls
            .first()
            .and_then(|call| {
                self.rows
                    .iter()
                    .find(|r| r["kind"] == "toolCall" && r["toolCallId"] == call["id"])
            })
            .and_then(|row| row["createdAt"].as_i64())
            .unwrap_or(self.fallback)
    }
}

/// 工具结果文本：媒体结果（附件引用数组）按 TS modelMessageContentToText 的占位形式呈现。
fn tool_text(content: &Value) -> Value {
    let Some(parts) = content.as_array() else {
        return content.clone();
    };
    parts
        .iter()
        .map(|p| match p["type"].as_str() {
            Some("text") => p["text"].as_str().unwrap_or_default().to_owned(),
            _ => {
                let mime = p["asset"]["mediaType"].as_str().unwrap_or_default();
                match p["name"].as_str().filter(|n| !n.is_empty()) {
                    Some(name) => format!("[Attached {mime}: {name}]"),
                    None => format!("[Attached {mime}]"),
                }
            }
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .into()
}
