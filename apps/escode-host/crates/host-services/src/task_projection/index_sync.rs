//! 会话写路径的索引同步（TS `escodeTaskIndexSyncer.ts` 的 `syncSnapshotAndBroadcast` 索引部分）。
//!
//! 与读路径的 [`super::meta::snapshot_to_meta`]（TS 适配器 `snapshotToMeta`）**不是同一个函数**：
//! session 服务的写入口（createSession/resumeSession/setModel）在 TS 走的是 syncer 的
//! `buildMetaFromSnapshot`，多出三处可见语义（评审 A12 核对时发现原实现借用了读路径投影）：
//!
//! 1. `modelOverride` / `thoughtLevelOverride`（trim 后非空才生效）覆盖快照值；
//! 2. 快照 `projection` **显式带 `target: null`** 时把索引里的目标清掉（缺席才是「不覆盖」）；
//! 3. 同步写 `searchableText`（可见正文，供全文搜索；200 000 UTF-16 单元截断）。

use serde_json::Value;

use super::meta::snapshot_to_meta;
use super::policy::js_trim;
use super::visible;
use crate::tasks_index::{SyncOptions, TasksIndex, TasksIndexError};

/// TS `TASK_SEARCH_TEXT_MAX_CHARS`（UTF-16 单元）。
const TASK_SEARCH_TEXT_MAX_UNITS: usize = 200_000;

/// 写路径覆盖值（TS `syncSnapshotAndBroadcast` 的 options）。
#[derive(Clone, Debug, Default)]
pub struct IndexOverrides {
    pub model: Option<String>,
    pub thought_level: Option<String>,
}

/// TS syncer `buildMetaFromSnapshot`。
pub fn index_meta_from_snapshot(snapshot: &Value, overrides: &IndexOverrides) -> Value {
    let mut meta = snapshot_to_meta(snapshot);
    let Some(record) = meta.as_object_mut() else {
        return meta;
    };
    let trimmed = |value: &Option<String>| {
        value
            .as_deref()
            .map(js_trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    if let Some(model) = trimmed(&overrides.model) {
        record.insert("model".to_owned(), Value::from(model));
    }
    if let Some(level) = trimmed(&overrides.thought_level) {
        record.insert("thoughtLevel".to_owned(), Value::from(level));
    }
    // `target` 键在场才覆盖：null = DB 明确没有 goal，需要清空索引里的目标。
    if snapshot
        .get("projection")
        .and_then(Value::as_object)
        .and_then(|projection| projection.get("target"))
        .is_some_and(Value::is_null)
    {
        record.insert("target".to_owned(), Value::Null);
    }
    meta
}

/// TS `buildSearchableTextFromSnapshot`：可见消息的 text part（assistant 只取最后一段），
/// trim 后非空才收；累计到上限即停；`join("\n")` 后按 UTF-16 单元截断。
pub fn searchable_text_from_snapshot(snapshot: &Value) -> String {
    let messages = snapshot
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut parts: Vec<String> = Vec::new();
    let mut total = 0_usize;
    'messages: for message in visible::user_visible_messages(messages) {
        let texts: Vec<&str> = message
            .get("parts")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        if texts.is_empty() {
            continue;
        }
        let is_assistant = message
            .get("info")
            .and_then(|info| info.get("role"))
            .and_then(Value::as_str)
            == Some("assistant");
        let chosen = if is_assistant {
            &texts[texts.len() - 1..]
        } else {
            &texts[..]
        };
        for text in chosen {
            let content = js_trim(text);
            if content.is_empty() {
                continue;
            }
            total += content.encode_utf16().count();
            parts.push(content.to_owned());
            if total >= TASK_SEARCH_TEXT_MAX_UNITS {
                break 'messages;
            }
        }
    }
    truncate_utf16(&parts.join("\n"), TASK_SEARCH_TEXT_MAX_UNITS)
}

/// JS `String.prototype.slice(0, n)`（UTF-16 单元）。截断点落在代理对中间时 TS 会留下孤立代理项，
/// Rust 字符串无法表示，这里退到该码点之前（只在第 200 000 个单元恰好劈开一个非 BMP 字符时可见）。
fn truncate_utf16(text: &str, limit: usize) -> String {
    let mut units = 0_usize;
    for (offset, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if next > limit {
            return text[..offset].to_owned();
        }
        units = next;
    }
    text.to_owned()
}

/// TS `syncTaskIndexSnapshot` / `syncSnapshotAndBroadcast` 的索引写：`subagent_child` 不写索引。
pub fn sync_snapshot_to_index(
    index: &TasksIndex,
    snapshot: &Value,
    overrides: &IndexOverrides,
) -> Result<Value, TasksIndexError> {
    let meta = index_meta_from_snapshot(snapshot, overrides);
    let is_subagent_child = snapshot
        .get("session")
        .and_then(|session| session.get("sessionKind"))
        .and_then(Value::as_str)
        == Some("subagent_child");
    if is_subagent_child {
        return Ok(meta);
    }
    match meta.as_object() {
        Some(record) => index.sync_task_meta(
            record,
            &SyncOptions {
                searchable_text: Some(searchable_text_from_snapshot(snapshot)),
                ..SyncOptions::default()
            },
        ),
        None => Ok(meta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(projection: Value, messages: Value) -> Value {
        json!({
            "session": {"sessionId": "s1", "traceId": "t1", "title": "T", "createdAt": 1, "updatedAt": 2,
                "mode": "build", "status": "idle", "model": {"providerId": "p", "modelId": "m"},
                "workspace": {"workspaceKey": "/w", "workspacePath": "/w"}},
            "settings": {"thoughtLevel": {"current": "high", "enabled": true, "available": []}},
            "projection": projection,
            "messages": messages,
        })
    }

    #[test]
    fn overrides_apply_after_trim_and_blank_overrides_are_ignored() {
        let base = snapshot(json!({"status": "idle"}), json!([]));
        let plain = index_meta_from_snapshot(&base, &IndexOverrides::default());
        let overridden = index_meta_from_snapshot(
            &base,
            &IndexOverrides {
                model: Some("  custom:x:y ".to_owned()),
                thought_level: Some("max".to_owned()),
            },
        );
        assert_eq!(overridden["model"], "custom:x:y");
        assert_eq!(overridden["thoughtLevel"], "max");
        let blank = index_meta_from_snapshot(
            &base,
            &IndexOverrides {
                model: Some("   ".to_owned()),
                thought_level: Some(String::new()),
            },
        );
        assert_eq!(blank["model"], plain["model"]);
        assert_eq!(blank["thoughtLevel"], "high");
    }

    #[test]
    fn explicit_null_target_clears_and_absent_target_does_not() {
        let cleared = index_meta_from_snapshot(
            &snapshot(json!({"status": "idle", "target": null}), json!([])),
            &IndexOverrides::default(),
        );
        assert_eq!(cleared.get("target"), Some(&Value::Null));
        let absent = index_meta_from_snapshot(
            &snapshot(json!({"status": "idle"}), json!([])),
            &IndexOverrides::default(),
        );
        assert!(absent.get("target").is_none());
    }

    #[test]
    fn searchable_text_takes_user_texts_and_the_last_assistant_text() {
        let messages = json!([
            {"info": {"role": "user"}, "parts": [{"type": "text", "text": " hello "}, {"type": "text", "text": "\u{feff}"}]},
            {"info": {"role": "assistant"}, "parts": [
                {"type": "reasoning", "text": "hidden"},
                {"type": "text", "text": "draft"},
                {"type": "text", "text": "final answer"}
            ]},
            {"info": {"role": "user"}, "parts": [{"type": "tool", "text": "x"}]}
        ]);
        let text = searchable_text_from_snapshot(&snapshot(json!({}), messages));
        assert_eq!(text, "hello\nfinal answer");
    }

    #[test]
    fn utf16_truncation_never_splits_a_code_point() {
        assert_eq!(truncate_utf16("ab😀c", 3), "ab");
        assert_eq!(truncate_utf16("ab😀c", 4), "ab😀");
        assert_eq!(truncate_utf16("abc", 10), "abc");
    }
}
