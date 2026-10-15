//! 相邻 assistant 的合并（TS `packages/shared/src/escodePersistedMessageMerge.ts`）。
//!
//! ESCode runtime 把同一个 user turn 里每一轮 LLM 调用落成独立 assistant 消息；持久化投影按
//! 「一个 turn 一条 assistant」的不变式把它们合并回去，并顺手把 `turnIndex` 显式按 user 计数赋值。
//! 从 `messages.rs` 拆出只为守住单文件 400 行上限；`coalesceConsecutiveESCodeAssistants`
//! 的判定与调用点在 `messages.rs`。

use serde_json::{Map, Value};

use super::message_parts::{
    Field, arith_number, array_field, field, js_number, part_type, strict_eq, truthy,
};

/// TS 收尾 map：`turnIndex` 按 user 计数赋值，`turn < 0` 取 0，只有不同才写。
pub(super) fn retag_turn_indexes(messages: Vec<Value>) -> Vec<Value> {
    let mut turn: i64 = -1;
    let mut tagged = Vec::with_capacity(messages.len());
    for message in messages {
        if field(&message, "role").and_then(Value::as_str) == Some("user") {
            turn += 1;
        }
        let effective = turn.max(0);
        if field(&message, "turnIndex").and_then(Value::as_f64) == Some(effective as f64) {
            tagged.push(message);
            continue;
        }
        let mut record = match message {
            Value::Object(record) => record,
            _ => Map::new(),
        };
        record.insert("turnIndex".to_owned(), Value::from(effective));
        tagged.push(Value::Object(record));
    }
    tagged
}

/// TS `mergeAssistantPair`：`{...first}` 起手，再按 TS 顺序覆盖各键。
pub(super) fn merge_assistant_pair(first: &Value, next: &Value) -> Value {
    let first_tools = array_field(first, "tools");
    let tool_index_offset = first_tools.len();
    let mut tools: Vec<Value> = first_tools.to_vec();
    tools.extend(array_field(next, "tools").iter().cloned());
    let mut parts: Vec<Value> = array_field(first, "parts").to_vec();
    parts.extend(
        array_field(next, "parts")
            .iter()
            .map(|part| shift_tool_index(part, tool_index_offset)),
    );
    // TS `first.content + next.content` 与 `(first.thought ?? "") + (next.thought ?? "")`。
    let content = concat_text(first, next, "content");
    let thought = match (field(first, "thought"), field(next, "thought")) {
        (None, None) => None,
        _ => Some(Value::from(concat_text(first, next, "thought"))),
    };
    // 任一轮还没有 completed time 就视为整 turn 未结束，duration 留空让 UI 显示「工作中」。
    let duration = match (field(first, "durationMs"), field(next, "durationMs")) {
        (Some(_first_duration), Some(next_duration)) => Some(js_number(js_max_zero(
            arith_number(field(next, "timestamp")) + arith_number(Some(next_duration))
                - arith_number(field(first, "timestamp")),
        ))),
        _ => None,
    };
    let character_count = match (
        field(first, "characterCount"),
        field(next, "characterCount"),
    ) {
        (None, None) => None,
        _ => Some(Value::from(js_utf16_len(&content))),
    };
    let merged_ids = collect_merged_message_ids(first, next);

    let mut output = match first {
        Value::Object(record) => record.clone(),
        _ => Map::new(),
    };
    // `{...first}` 之后再逐键覆盖；`None` 等价于 TS 的 `undefined`（键消失）。
    let updates: Vec<Field> = vec![
        ("content", Some(Value::from(content))),
        ("model", overlay(first, next, "model")),
        ("durationMs", duration),
        ("characterCount", character_count),
        ("interrupted", overlay(first, next, "interrupted")),
        ("feedback", overlay(first, next, "feedback")),
        (
            "mergedMessageIds",
            (!merged_ids.is_empty()).then_some(Value::Array(merged_ids)),
        ),
        ("goalIteration", overlay(first, next, "goalIteration")),
        ("attachments", overlay(first, next, "attachments")),
        ("tools", (!tools.is_empty()).then_some(Value::Array(tools))),
        ("thought", thought),
        ("parts", (!parts.is_empty()).then_some(Value::Array(parts))),
        ("checkpointState", overlay(first, next, "checkpointState")),
        ("checkpointReason", overlay(first, next, "checkpointReason")),
        (
            "checkpointUpdatedAt",
            overlay(first, next, "checkpointUpdatedAt"),
        ),
        ("bodyRefs", overlay(first, next, "bodyRefs")),
        ("toolSlice", overlay(first, next, "toolSlice")),
    ];
    for (key, value) in updates {
        match value {
            Some(value) => output.insert(key.to_owned(), value),
            None => output.remove(key),
        };
    }
    Value::Object(output)
}

/// TS `a ?? b`：`null` 与缺值都算「无值」，只有非 `null` 的 `next` 才覆盖。
fn overlay(first: &Value, next: &Value, key: &str) -> Option<Value> {
    match field(next, key) {
        Some(value) if !value.is_null() => Some(value.clone()),
        _ => match field(first, key) {
            Some(value) if !value.is_null() => Some(value.clone()),
            _ => None,
        },
    }
}

/// TS `(first[k] ?? "") + (next[k] ?? "")`：`null`/缺值当空串，标量按 JS 字符串化。
fn concat_text(first: &Value, next: &Value, key: &str) -> String {
    let mut text = String::new();
    for value in [field(first, key), field(next, key)].into_iter().flatten() {
        match value {
            Value::String(part) => text.push_str(part),
            Value::Number(number) => text.push_str(&number.to_string()),
            Value::Bool(flag) => text.push_str(if *flag { "true" } else { "false" }),
            _ => {}
        }
    }
    text
}

/// TS `{ type: "tool-call", toolIndex: part.toolIndex + offset }`（非 tool-call 原样保留）。
fn shift_tool_index(part: &Value, offset: usize) -> Value {
    if part_type(part) != Some("tool-call") {
        return part.clone();
    }
    let index = arith_number(field(part, "toolIndex")) + offset as f64;
    let mut shifted = Map::new();
    shifted.insert("type".to_owned(), Value::from("tool-call"));
    shifted.insert("toolIndex".to_owned(), js_number(index));
    Value::Object(shifted)
}

/// TS `collectMergedMessageIds`：两侧 id 与 `mergedMessageIds` 拼接、真值过滤、去重保序。
fn collect_merged_message_ids(first: &Value, next: &Value) -> Vec<Value> {
    let mut candidates: Vec<Option<&Value>> = vec![field(first, "id")];
    candidates.extend(array_field(first, "mergedMessageIds").iter().map(Some));
    candidates.push(field(next, "id"));
    candidates.extend(array_field(next, "mergedMessageIds").iter().map(Some));
    let mut ids: Vec<Value> = Vec::new();
    for candidate in candidates.into_iter().flatten() {
        let known = ids.iter().any(|existing| strict_eq(existing, candidate));
        if known || !truthy(candidate) {
            continue;
        }
        ids.push(candidate.clone());
    }
    ids
}

/// JS 字符串 `.length`：UTF-16 码元数（emoji 记 2），不是 Rust 的码点数。
fn js_utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// TS `Math.max(x, 0)`：`NaN` 保持 `NaN`（Rust 的 `f64::max` 会吞掉它）。
fn js_max_zero(value: f64) -> f64 {
    if value.is_nan() {
        value
    } else {
        value.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::merge_assistant_pair;
    use serde_json::json;

    #[test]
    fn merge_shifts_tool_index_and_keeps_unfinished_duration_empty() {
        let first = json!({"id": "a1", "role": "assistant", "content": "one", "timestamp": 10,
            "durationMs": 5, "characterCount": 3, "tools": [{"toolName": "Bash"}],
            "parts": [{"type": "content", "content": "one"}, {"type": "tool-call", "toolIndex": 0}]});
        let next = json!({"id": "a2", "role": "assistant", "content": "two", "timestamp": 20,
            "tools": [{"toolName": "Read"}],
            "parts": [{"type": "tool-call", "toolIndex": 0}]});
        let merged = merge_assistant_pair(&first, &next);
        assert_eq!(merged.get("content"), Some(&json!("onetwo")));
        assert_eq!(merged.get("timestamp"), Some(&json!(10)));
        // 任一轮缺 durationMs → 整 turn 仍未结束，不留半个时长；model 回落到 first。
        assert_eq!(merged.get("durationMs"), None);
        assert_eq!(merged.get("characterCount"), Some(&json!(6)));
        assert_eq!(
            merged.get("parts"),
            Some(&json!([
                {"type": "content", "content": "one"},
                {"type": "tool-call", "toolIndex": 0},
                {"type": "tool-call", "toolIndex": 1}
            ]))
        );
        assert_eq!(merged.get("mergedMessageIds"), Some(&json!(["a1", "a2"])));
        assert_eq!(
            merged
                .get("tools")
                .and_then(|tools| tools.as_array())
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn merge_uses_next_side_values_and_recomputes_duration() {
        let first = json!({"id": "a1", "role": "assistant", "content": "a", "timestamp": 10,
            "durationMs": 5, "thought": "t1", "model": "glm/a"});
        let next = json!({"id": "a2", "role": "assistant", "content": "b", "timestamp": 20,
            "durationMs": 5, "thought": "t2", "model": "glm/b", "mergedMessageIds": ["a1"]});
        let merged = merge_assistant_pair(&first, &next);
        assert_eq!(merged.get("durationMs"), Some(&json!(15)));
        assert_eq!(merged.get("thought"), Some(&json!("t1t2")));
        assert_eq!(merged.get("model"), Some(&json!("glm/b")));
        // 去重：`a1` 既来自 first.id 也来自 next.mergedMessageIds。
        assert_eq!(merged.get("mergedMessageIds"), Some(&json!(["a1", "a2"])));
    }
}
