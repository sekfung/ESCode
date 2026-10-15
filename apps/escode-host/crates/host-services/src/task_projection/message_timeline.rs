//! `mapMessage` 的 synthetic timeline 合成：fork notice 与 compaction divider。
//!
//! 从 `message_parts.rs` 拆出只为守住单文件 400 行上限。

use serde_json::Value;

use super::message_parts::{
    Field, build_object, field, number_clone, string_value, truthy, truthy_field,
    truthy_field_value,
};

/// TS `extractSyntheticTimelineFromTextPart`：fork notice 把 timeline meta 放在 `metadata.forkContext`。
pub(super) fn extract_synthetic_timeline_from_text_part(part: &Value) -> Option<Value> {
    let metadata = field(part, "metadata")?;
    if !metadata.is_object() {
        return None;
    }
    let context = field(metadata, "forkContext")?;
    if !context.is_object()
        || field(context, "kind").and_then(Value::as_str) != Some("session_fork")
    {
        return None;
    }
    let parent = field(context, "parentSessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let target = field(context, "targetMessageId")
        .and_then(Value::as_str)
        .unwrap_or("");
    if parent.is_empty() || target.is_empty() {
        return None;
    }
    Some(build_object([
        ("version", Some(Value::from(1))),
        ("kind", Some(Value::from("synthetic"))),
        ("type", Some(Value::from("session_fork"))),
        ("display", Some(Value::from("separator"))),
        ("parentSessionId", Some(Value::from(parent))),
        ("targetMessageId", Some(Value::from(target))),
        (
            "targetCheckpointId",
            string_value(field(context, "targetCheckpointId")).map(Value::from),
        ),
        (
            "restoredFileCount",
            number_clone(field(context, "restoredFileCount")),
        ),
    ]))
}

/// TS `synthesizeCompactionTimeline`：只有 UI 横线才合成；compact summary 的模型上下文不合成。
pub(super) fn synthesize_compaction_timeline(part: &Value) -> Option<Value> {
    let metadata = field(part, "metadata").unwrap_or(&Value::Null);
    let operation_id = string_value(field(metadata, "operationId"))
        .map(Value::from)
        .or_else(|| field(part, "partId").cloned());
    let statuses = [
        "started",
        "retrying",
        "skipped",
        "completed",
        "failed",
        "interrupted",
    ];
    let status = enum_text(field(metadata, "timelineStatus"), &statuses);
    let part_summary = truthy_field_value(part, "summaryMessageId");
    if status.is_none() && part_summary.is_none() {
        // compact summary user message 也带 compaction metadata，但它是模型上下文，不是 UI timeline。
        return None;
    }
    let triggers = ["manual", "auto", "reactive", "partial", "session_memory"];
    let trigger = match enum_text(field(metadata, "trigger"), &triggers) {
        Some(trigger) => Value::from(trigger),
        None => Value::from(if truthy_field(part, "auto") {
            "auto"
        } else {
            "manual"
        }),
    };
    // TS 用 `??`：只有 `part.reason` 缺失或为 `null` 才看 `metadata.reason`。
    let reason = match field(part, "reason") {
        Some(reason) if !reason.is_null() => Some(reason.clone()),
        _ => string_value(field(metadata, "reason")).map(Value::from),
    }
    .filter(truthy);
    let boundary_id = string_value(field(metadata, "boundaryId"))
        .map(Value::from)
        .or_else(|| part_summary.cloned())
        .filter(truthy);
    let summary_message_id = part_summary
        .cloned()
        .or_else(|| string_value(field(metadata, "summaryMessageId")).map(Value::from))
        .filter(truthy);
    let mut fields: Vec<Field> = vec![
        ("version", Some(Value::from(1))),
        ("kind", Some(Value::from("synthetic"))),
        ("type", Some(Value::from("context_compaction"))),
        ("operationId", operation_id),
        ("status", Some(Value::from(status.unwrap_or("completed")))),
        ("trigger", Some(trigger)),
        ("display", Some(Value::from("separator"))),
        (
            "replace",
            field(metadata, "replace")
                .and_then(Value::as_bool)
                .map(Value::from),
        ),
        ("reason", reason),
        ("boundaryId", boundary_id),
        ("summaryMessageId", summary_message_id),
    ];
    // 只在 TS `numberValue(metadata.x) !== undefined` 时出现的可选数字字段。
    for key in [
        "preCompactTokenCount",
        "postCompactTokenCount",
        "truePostCompactTokenCount",
    ] {
        fields.push((key, number_clone(field(metadata, key))));
    }
    for key in ["attempt", "maxAttempts", "startedAt", "endedAt"] {
        fields.push((key, number_clone(field(metadata, key))));
    }
    Some(build_object(fields))
}

/// TS `timelineStatusValue` / `timelineTriggerValue`：只接受白名单里的字符串。
fn enum_text<'a>(value: Option<&'a Value>, allowed: &[&str]) -> Option<&'a str> {
    value
        .and_then(Value::as_str)
        .filter(|text| allowed.contains(text))
}
