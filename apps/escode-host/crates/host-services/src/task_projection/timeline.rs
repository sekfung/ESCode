//! goal verifier / session fork 的 timeline 投影（task facade 的 synthetic divider）。
//!
//! 上游真相源：`packages/services/src/escode-agent/escodeTaskServiceAdapter.ts` 的
//! `addSessionForkSnapshotFallback`、`addGoalVerificationTimelineSnapshotFallback`、
//! `normalizeGoalVerificationTimelineMessageOrder`、`insertGoalVerificationTimelineMessages`
//! 及 `goalVerificationAnchorInsertIndex` / `timestampInsertIndex` /
//! `findPersistedMessageIndexById` / `getSnapshotGoalActiveIterationCount`。
//!
//! 这些函数作用在**已映射**的 `ESCodePersistedMessage` 上（`id`/`timestamp`/
//! `syntheticTimeline` 键），所以必须在 `map_message` 之后调用。

use serde_json::{Map, Value};

use super::visible;

/// TS `addSessionForkSnapshotFallback`：旧 fork 会话没有落库 bot divider 时补一条。
pub fn add_session_fork_fallback(messages: Vec<Value>, snapshot: &Value) -> Vec<Value> {
    let parent_session_id = field(snapshot, "session")
        .and_then(|session| field(session, "parentSessionId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if parent_session_id.is_empty()
        || messages.iter().any(|message| {
            field(message, "syntheticTimeline")
                .and_then(|timeline| field(timeline, "type"))
                .and_then(Value::as_str)
                == Some("session_fork")
        })
    {
        return messages;
    }
    let created_at = field(snapshot, "session")
        .and_then(|session| field(session, "createdAt"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut fork = Map::new();
    fork.insert(
        "id".to_owned(),
        Value::from(format!("escode-timeline-fork-{parent_session_id}-")),
    );
    fork.insert("role".to_owned(), Value::from("user"));
    fork.insert("content".to_owned(), Value::from(""));
    fork.insert("timestamp".to_owned(), created_at);
    // 旧的纯对话 fork 没有落库 synthetic notice，只能从 session.parentSessionId 恢复一个
    // 不可跳转的分割线，避免历史 fork 会话完全看不到来源边界。
    fork.insert(
        "syntheticTimeline".to_owned(),
        serde_json::json!({
            "version": 1,
            "kind": "synthetic",
            "type": "session_fork",
            "display": "separator",
            "parentSessionId": parent_session_id,
            "targetMessageId": "",
        }),
    );
    let mut extended = messages;
    extended.push(Value::Object(fork));
    extended
}

/// TS `addGoalVerificationTimelineSnapshotFallback`：agent snapshot 的 verifier 生命周期补 divider。
pub fn add_goal_verification_timeline_fallback(
    messages: Vec<Value>,
    snapshot: &Value,
) -> Vec<Value> {
    let timeline = field(snapshot, "runtime")
        .and_then(|runtime| field(runtime, "goalVerificationTimeline"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if timeline.is_empty() {
        return messages;
    }
    let existing = messages
        .iter()
        .filter_map(|message| {
            let timeline = field(message, "syntheticTimeline")?;
            (field(timeline, "type").and_then(Value::as_str) == Some("goal_verification"))
                .then(|| identity_key(timeline))
        })
        .collect::<Vec<_>>();
    let mut timeline_messages = Vec::new();
    for item in timeline {
        if existing.contains(&identity_key(item)) {
            continue;
        }
        let mut message = Map::new();
        message.insert("id".to_owned(), Value::from(message_id(item)));
        message.insert("role".to_owned(), Value::from("assistant"));
        message.insert("content".to_owned(), Value::from(""));
        message.insert(
            "timestamp".to_owned(),
            field(item, "startedAt")
                .or_else(|| field(item, "updatedAt"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        // goal verifier lifecycle 是 agent snapshot 的持久状态，不一定有对应 message history。
        message.insert("syntheticTimeline".to_owned(), item.clone());
        timeline_messages.push(Value::Object(message));
    }
    if timeline_messages.is_empty() {
        return messages;
    }
    insert_goal_verification_timeline_messages(messages, timeline_messages)
}

/// TS `normalizeGoalVerificationTimelineMessageOrder`：已存在的 divider 也按 anchor 重排。
pub fn normalize_goal_verification_timeline_order(messages: Vec<Value>) -> Vec<Value> {
    let (timeline_messages, rest): (Vec<Value>, Vec<Value>) =
        messages.into_iter().partition(is_goal_verification);
    if timeline_messages.is_empty() {
        return rest;
    }
    insert_goal_verification_timeline_messages(rest, timeline_messages)
}

/// TS `getSnapshotGoalActiveIterationCount`：按 target 过滤 verifier timeline 后的活跃轮次上限。
pub fn snapshot_goal_active_iteration_count(snapshot: &Value) -> u32 {
    let target = field(snapshot, "projection").and_then(|projection| field(projection, "target"));
    let target_id = target
        .and_then(|target| field(target, "targetId"))
        .and_then(Value::as_str);
    let timeline = field(snapshot, "runtime")
        .and_then(|runtime| field(runtime, "goalVerificationTimeline"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    target_id.is_none_or(|id| {
                        field(item, "targetId").and_then(Value::as_str) == Some(id)
                    })
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let status = target
        .and_then(|target| field(target, "status"))
        .and_then(Value::as_str);
    visible::goal_active_iteration_count(status, Some(&timeline))
}

fn insert_goal_verification_timeline_messages(
    messages: Vec<Value>,
    timeline_messages: Vec<Value>,
) -> Vec<Value> {
    let mut result = messages;
    let mut ordered = timeline_messages;
    ordered.sort_by(|left, right| {
        let left = number_field(left, "timestamp");
        let right = number_field(right, "timestamp");
        left.partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for message in ordered {
        let anchor = field(&message, "syntheticTimeline")
            .filter(|timeline| {
                field(timeline, "type").and_then(Value::as_str) == Some("goal_verification")
            })
            .and_then(|timeline| field(timeline, "anchorAssistantMessageId"))
            .and_then(Value::as_str);
        let anchor_index = anchor.and_then(|anchor| find_message_index_by_id(&result, anchor));
        let index = match anchor_index {
            Some(anchor_index) => anchor_insert_index(&result, anchor_index),
            None => timestamp_insert_index(&result, number_field(&message, "timestamp")),
        };
        result.insert(index.min(result.len()), message);
    }
    result
}

/// TS `goalVerificationAnchorInsertIndex`：anchor 之后跳过已排好的 verifier divider。
fn anchor_insert_index(messages: &[Value], anchor_index: usize) -> usize {
    let mut index = anchor_index + 1;
    while index < messages.len() && is_goal_verification(&messages[index]) {
        index += 1;
    }
    index
}

/// TS `timestampInsertIndex`：第一条更晚的消息之前；没有就追加。
fn timestamp_insert_index(messages: &[Value], timestamp: f64) -> usize {
    messages
        .iter()
        .position(|message| number_field(message, "timestamp") > timestamp)
        .unwrap_or(messages.len())
}

/// TS `findPersistedMessageIndexById`：`id` 命中或落在 `mergedMessageIds` 里。
fn find_message_index_by_id(messages: &[Value], message_id: &str) -> Option<usize> {
    messages.iter().position(|message| {
        field(message, "id").and_then(Value::as_str) == Some(message_id)
            || field(message, "mergedMessageIds")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(message_id)))
    })
}

fn is_goal_verification(message: &Value) -> bool {
    field(message, "syntheticTimeline")
        .and_then(|timeline| field(timeline, "type"))
        .and_then(Value::as_str)
        == Some("goal_verification")
}

/// TS `goalVerificationTimelineIdentityKey`：有迭代号按 target+迭代号，否则按 verificationId。
fn identity_key(item: &Value) -> String {
    if field(item, "goalIteration").is_some_and(Value::is_number) {
        let target = field(item, "targetId")
            .and_then(Value::as_str)
            .unwrap_or("");
        format!(
            "{target}:{}",
            number_text(number_field(item, "goalIteration"))
        )
    } else {
        format!(
            "verification:{}",
            field(item, "verificationId")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
    }
}

/// TS `goalVerificationTimelineMessageId`。
fn message_id(item: &Value) -> String {
    if field(item, "goalIteration").is_some_and(Value::is_number) {
        let target = field(item, "targetId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let iteration = number_text(number_field(item, "goalIteration"));
        return format!("escode-goal-verification-{target}-{iteration}");
    }
    format!(
        "escode-goal-verification-{}",
        field(item, "verificationId")
            .and_then(Value::as_str)
            .unwrap_or("")
    )
}

fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn number_field(value: &Value, key: &str) -> f64 {
    field(value, key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn persisted(id: &str, timestamp: f64, role: &str) -> Value {
        json!({"id": id, "role": role, "content": "", "timestamp": timestamp})
    }

    #[test]
    fn session_fork_fallback_appends_a_single_separator() {
        let snapshot = json!({"session": {"parentSessionId": "parent_1", "createdAt": 500}});
        let messages = add_session_fork_fallback(vec![persisted("m1", 100.0, "user")], &snapshot);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["id"], json!("escode-timeline-fork-parent_1-"));
        assert_eq!(messages[1]["timestamp"], json!(500));
        assert_eq!(
            messages[1]["syntheticTimeline"]["type"],
            json!("session_fork")
        );
        // 已有 fork divider 时不重复补；无 parentSessionId 时不补。
        let already = add_session_fork_fallback(messages, &snapshot);
        assert_eq!(already.len(), 2);
        assert_eq!(
            add_session_fork_fallback(
                vec![persisted("m1", 100.0, "user")],
                &json!({"session": {}})
            )
            .len(),
            1
        );
    }

    #[test]
    fn goal_verification_fallback_inserts_after_anchor_and_dedupes() {
        let snapshot = json!({
            "runtime": {"goalVerificationTimeline": [
                {"type": "goal_verification", "targetId": "t1", "goalIteration": 1,
                 "startedAt": 150, "updatedAt": 160, "anchorAssistantMessageId": "a1",
                 "status": "completed"},
                {"type": "goal_verification", "verificationId": "v2", "targetId": "t1",
                 "startedAt": 400, "updatedAt": 410, "status": "started"},
            ]},
        });
        let messages = vec![
            persisted("u1", 100.0, "user"),
            persisted("a1", 120.0, "assistant"),
            persisted("u2", 300.0, "user"),
        ];
        let projected = add_goal_verification_timeline_fallback(messages, &snapshot);
        let ids = projected
            .iter()
            .map(|message| message["id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        // 迭代号那条插在 anchor 之后；无 anchor 的那条按时间戳插到 u2 之前。
        // t1-1 有 anchor（插在 a1 之后）；v2 无 anchor → 按时间戳（400）落在 u2（300）之后。
        assert_eq!(
            ids,
            vec![
                "u1",
                "a1",
                "escode-goal-verification-t1-1",
                "u2",
                "escode-goal-verification-v2"
            ]
        );
        // 幂等：再投影一次不新增（identity key 去重）。
        let again = add_goal_verification_timeline_fallback(projected.clone(), &snapshot);
        assert_eq!(again.len(), projected.len());
    }

    #[test]
    fn normalize_reorders_existing_dividers_by_anchor() {
        let messages = [
            persisted("u1", 100.0, "user"),
            persisted("a1", 120.0, "assistant"),
            json!({"id": "verifier", "role": "assistant", "content": "", "timestamp": 130.0,
                   "syntheticTimeline": {"type": "goal_verification", "targetId": "t1",
                     "goalIteration": 1, "anchorAssistantMessageId": "a1"}}),
            persisted("u2", 200.0, "user"),
        ];
        let shuffled = vec![
            persisted("u1", 100.0, "user"),
            persisted("a1", 120.0, "assistant"),
            persisted("u2", 200.0, "user"),
            messages[2].clone(),
        ];
        let normalized = normalize_goal_verification_timeline_order(shuffled);
        let ids = normalized
            .iter()
            .map(|message| message["id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["u1", "a1", "verifier", "u2"]);
        // 没有 divider 时原样返回。
        let plain = vec![persisted("u1", 100.0, "user")];
        assert_eq!(normalize_goal_verification_timeline_order(plain).len(), 1);
    }

    #[test]
    fn active_iteration_count_filters_by_target_id() {
        let snapshot = json!({
            "projection": {"target": {"targetId": "t1", "status": "active"}},
            "runtime": {"goalVerificationTimeline": [
                {"type": "goal_verification", "targetId": "t1", "goalIteration": 1,
                 "status": "completed", "verification": {"passed": true}},
                {"type": "goal_verification", "targetId": "other", "goalIteration": 9,
                 "status": "started"},
            ]},
        });
        // 只看 t1 的 timeline：最后一条 completed+passed → 收敛到 1。
        assert_eq!(snapshot_goal_active_iteration_count(&snapshot), 1);
        // target 缺失 → targetStatus 为 null → 0（TS 同：`target?.status ?? null`）。
        let no_target = json!({
            "projection": {},
            "runtime": {"goalVerificationTimeline": [
                {"type": "goal_verification", "targetId": "t1", "goalIteration": 1,
                 "status": "started"},
            ]},
        });
        assert_eq!(snapshot_goal_active_iteration_count(&no_target), 0);
        // 没有 timeline：target 有 status 时算 1 轮；target 缺 status（或整个缺失）→ 0。
        assert_eq!(
            snapshot_goal_active_iteration_count(
                &json!({"projection": {"target": {"status": "active"}}})
            ),
            1
        );
        assert_eq!(
            snapshot_goal_active_iteration_count(&json!({"projection": {"target": {}}})),
            0
        );
        assert_eq!(snapshot_goal_active_iteration_count(&json!({})), 0);
    }
}
