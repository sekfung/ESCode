//! Host 任务域快照读面的消息投影：TS 服务层投影的 Rust 等价实现。
//!
//! 上游真相源：
//! - `packages/services/src/escode-agent/escodeTaskServiceAdapter.ts`：`mapMessage` / `mapToolPart` /
//!   `attachToolCallIdToRaw` / `textFromParts` / `reasoningFromParts` 及文件尾部小工具；
//! - `packages/shared/src/escode-protocol-legacy-types.ts`：`textFromESCodeMessageParts`；
//! - `packages/shared/src/escodePersistedMessageMerge.ts`：`coalesceConsecutiveESCodeAssistants`
//!   （合并实现在 `message_merge.rs`）。
//!
//! TS 里值为 `undefined` 的键在 JSON 里消失，这里对应「不插入该键」，不写 `null`
//! （TS 显式写 `null` 的位置除外，例如 `NaN` 经 `JSON.stringify`）；键按 TS 对象字面量顺序插入
//! （`serde_json` 未开 `preserve_order` 时序列化按键排序，JSON 对象无序，语义不变）。
//! 附件反投影与 synthetic timeline 合成在 `message_parts.rs`。

use std::collections::BTreeMap;

use serde_json::Value;

use super::background;
use super::message_merge::{merge_assistant_pair, retag_turn_indexes};
use super::message_parts::{
    Field, arith_number, build_object, field, format_model_selection, js_number,
    map_prompt_attachments_from_parts, part_type, role_of, strict_eq_optional, string_value,
    truthy_field, truthy_field_value,
};
use super::message_timeline::{
    extract_synthetic_timeline_from_text_part, synthesize_compaction_timeline,
};

/// TS `textFromESCodeMessageParts`：`type == "text"` 的 `text` 顺序连接（不 trim、不加分隔）。
pub fn text_from_parts(parts: &[Value]) -> String {
    parts
        .iter()
        .filter(|part| part_type(part) == Some("text"))
        .filter_map(|part| field(part, "text").and_then(Value::as_str))
        .collect()
}

/// TS `reasoningFromParts`：`type == "reasoning"` 的 `text` 顺序连接，空串按 TS 给 `undefined`。
pub fn reasoning_from_parts(parts: &[Value]) -> Option<String> {
    let text: String = parts
        .iter()
        .filter(|part| part_type(part) == Some("reasoning"))
        .filter_map(|part| field(part, "text").and_then(Value::as_str))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// TS `mapMessage`：`ESCodeMessageWithParts` → `ESCodePersistedMessage`。
///
/// `id`/`role`/`content`/`timestamp` 恒写（来源缺值时按 TS 不插入该键）；
/// `model`/`syntheticTimeline`/`attachments`/`goalIteration`/`durationMs`/`thought`/`tools`/`parts`
/// 都是条件键，条件与 TS 的对象展开逐条一致。
pub fn map_message(
    message: &Value,
    goal_iteration: Option<u32>,
    background_task_notifications: Option<&BTreeMap<String, Value>>,
) -> Value {
    let info = field(message, "info").unwrap_or(&Value::Null);
    let parts = field(message, "parts").and_then(Value::as_array);
    let parts = parts.map_or(&[][..], Vec::as_slice);
    let time = field(info, "time").unwrap_or(&Value::Null);
    let role = field(info, "role").and_then(Value::as_str);

    // fork notice 等结构化 synthetic 消息把 timeline meta 写在 part 上，只取第一条命中的。
    let mut synthetic_timeline = None;
    for part in parts {
        synthetic_timeline = match part_type(part) {
            Some("text") => extract_synthetic_timeline_from_text_part(part),
            Some("compaction") => synthesize_compaction_timeline(part),
            _ => None,
        };
        if synthetic_timeline.is_some() {
            break;
        }
    }

    let mut tools: Vec<Value> = Vec::new();
    let mut ordered_parts: Vec<Value> = Vec::new();
    for part in parts {
        match part_type(part) {
            Some("text") => ordered_parts.push(text_part("content", part)),
            Some("reasoning") => ordered_parts.push(text_part("thought", part)),
            Some("tool") => {
                // toolIndex 取 push 之前的长度（TS 先 push tools 再 push part，索引同值）。
                ordered_parts.push(build_object([
                    ("type", Some(Value::from("tool-call"))),
                    ("toolIndex", Some(Value::from(tools.len()))),
                ]));
                tools.push(map_tool_part(part, background_task_notifications));
            }
            _ => {}
        }
    }

    let mut entries: Vec<Field> = vec![
        ("id", field(info, "messageId").cloned()),
        ("role", field(info, "role").cloned()),
        ("content", Some(Value::from(text_from_parts(parts)))),
        ("timestamp", field(time, "created").cloned()),
        (
            "model",
            truthy_field_value(info, "model")
                .map(format_model_selection)
                .map(Value::from),
        ),
        ("syntheticTimeline", synthetic_timeline),
        (
            "attachments",
            match role {
                Some("user") => map_prompt_attachments_from_parts(parts),
                _ => None,
            }
            .map(Value::Array),
        ),
    ];
    if role == Some("assistant") {
        entries.extend([
            (
                "goalIteration",
                goal_iteration
                    .filter(|iteration| *iteration != 0)
                    .map(Value::from),
            ),
            ("durationMs", message_duration(time)),
            ("thought", reasoning_from_parts(parts).map(Value::from)),
            ("tools", (!tools.is_empty()).then_some(Value::Array(tools))),
            (
                "parts",
                (!ordered_parts.is_empty()).then_some(Value::Array(ordered_parts)),
            ),
        ]);
    }
    build_object(entries)
}

/// TS `{ type: "content", content: part.text }` / `{ type: "thought", content: part.text }`。
fn text_part(kind: &'static str, part: &Value) -> Value {
    build_object([
        ("type", Some(Value::from(kind))),
        ("content", field(part, "text").cloned()),
    ])
}

/// TS `info.time.completed ? completed - created : undefined`。
///
/// TS 的 `?` 是非零即真：`completed === 0` 视为无值；`created` 缺失时 JS 减法得 `NaN`，
/// `JSON.stringify` 把 `NaN` 落成 `null`（`js_number` 同）。
fn message_duration(time: &Value) -> Option<Value> {
    truthy_field_value(time, "completed").map(|completed| {
        js_number(arith_number(Some(completed)) - arith_number(field(time, "created")))
    })
}

/// TS `mapToolPart`：`completed` / `error` / 其它三种 status 的字段组合各不相同，逐条照抄。
fn map_tool_part(part: &Value, notifications: Option<&BTreeMap<String, Value>>) -> Value {
    let state = field(part, "state").unwrap_or(&Value::Null);
    let tool = field(part, "tool").cloned();
    let call_id = field(part, "callId").and_then(Value::as_str);
    let notification = match (call_id, notifications) {
        (Some(call_id), Some(notifications)) => notifications.get(call_id),
        _ => None,
    };
    // TS `"metadata" in state ? (state.metadata ?? state) : state`：metadata 才是 raw 的载体。
    let raw_source = match field(state, "metadata") {
        Some(metadata) if !metadata.is_null() => metadata.clone(),
        _ => state.clone(),
    };
    let raw = background::attach_notification_to_raw(
        attach_tool_call_id_to_raw(raw_source, call_id),
        notification,
    );
    let title = truthy_field_value(state, "title")
        .cloned()
        .or_else(|| tool.clone());
    match field(state, "status").and_then(Value::as_str) {
        Some("completed") => {
            // snapshot 的 completed 是 background Agent launch ACK：failed notification 必须覆盖它，
            // 但 stopped 等既有语义不能被顺带改写。
            let failed = notification_status(notification) == Some("failed");
            let error = notification
                .filter(|_| failed)
                .and_then(|notice| string_value(field(notice, "error")))
                .map(Value::from);
            build_object([
                ("toolName", tool.clone()),
                ("title", title),
                ("kind", tool),
                (
                    "status",
                    Some(Value::from(if failed { "failed" } else { "completed" })),
                ),
                ("input", field(state, "input").cloned()),
                ("output", field(state, "output").cloned()),
                ("error", error),
                ("raw", Some(raw)),
            ])
        }
        Some("error") => build_object([
            ("toolName", tool.clone()),
            ("title", tool.clone()),
            ("kind", tool),
            ("status", Some(Value::from("failed"))),
            ("input", field(state, "input").cloned()),
            ("error", field(state, "error").cloned()),
            ("raw", Some(raw)),
        ]),
        _ => build_object([
            ("toolName", tool.clone()),
            ("title", title),
            ("kind", tool),
            ("input", field(state, "input").cloned()),
            ("raw", Some(raw)),
        ]),
    }
}

/// TS `taskNotification?.status ? escodeBackgroundTaskNotificationToolUpdateStatus(status) : undefined`。
fn notification_status(notification: Option<&Value>) -> Option<&'static str> {
    let status = truthy_field_value(notification.unwrap_or(&Value::Null), "status");
    status
        .map(|status| status.as_str().unwrap_or(""))
        .and_then(background::notification_tool_update_status)
}

/// TS `attachToolCallIdToRaw`：非对象（含数组）包一层；对象只在没有字符串 `toolCallId` 时补写。
fn attach_tool_call_id_to_raw(raw: Value, tool_call_id: Option<&str>) -> Value {
    let mut record = match raw {
        Value::Object(record) => record,
        other => {
            return build_object([
                ("toolCallId", tool_call_id.map(Value::from)),
                ("raw", Some(other)),
            ]);
        }
    };
    if !record.get("toolCallId").is_some_and(Value::is_string) {
        match tool_call_id {
            Some(id) => record.insert("toolCallId".to_owned(), Value::from(id)),
            None => record.remove("toolCallId"),
        };
    }
    Value::Object(record)
}

/// TS `coalesceConsecutiveESCodeAssistants`：合并相邻 assistant，再按 user 计数重写 `turnIndex`。
pub fn coalesce_consecutive_assistants(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    for message in messages {
        let same_turn = merged
            .last()
            .is_some_and(|last| mergeable_assistants(last, &message));
        match merged.pop() {
            Some(first) if same_turn => merged.push(merge_assistant_pair(&first, &message)),
            popped => {
                merged.extend(popped);
                merged.push(message);
            }
        }
    }
    retag_turn_indexes(merged)
}

/// TS 合并条件：相邻 assistant、都不是 synthetic timeline、`goalIteration` 严格相等。
fn mergeable_assistants(last: &Value, message: &Value) -> bool {
    role_of(last) == "assistant"
        && role_of(message) == "assistant"
        && !truthy_field(last, "syntheticTimeline")
        && !truthy_field(message, "syntheticTimeline")
        && strict_eq_optional(
            field(last, "goalIteration"),
            field(message, "goalIteration"),
        )
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
