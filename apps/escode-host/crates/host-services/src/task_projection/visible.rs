//! TS `packages/shared/src/escode-session-visible-content.ts` 的 Rust 投影：goal 提醒判定、可见消息
//! 过滤、goal 迭代号与会话标题回退（`textFromESCodeMessageParts` 在 `policy.rs`）。
//!
//! 差异：`slice(0, 80)` 按 UTF-16 码元切且可能留孤立代理元，Rust 无法表示 → 按「码元截断且不裂开
//! 代理对」实现；`String(...)`/`localeCompare` 同 `policy.rs`。

use super::policy::{
    self, GOAL_CONTINUATION_MARKER, GOAL_CONTINUATION_SOURCE, GOAL_REMINDER_PREFIX,
    GOAL_STATE_MARKER, Policy, js_trim, js_trim_start, js_truthy, metadata_map, metadata_source,
    opt_str, parts_of, text_from_parts,
};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::BTreeMap;

fn info(message: &Value) -> Option<&policy::Record> {
    message.get("info").and_then(Value::as_object)
}
fn is_user(message: &Value) -> bool {
    opt_str(info(message), "role") == Some("user")
}
/// `stringValue(message.info.metadata?.source)`。
fn info_metadata_source(message: &Value) -> Option<&str> {
    metadata_source(metadata_map(info(message).and_then(|r| r.get("metadata"))))
}

/// TS `isESCodeGoalContinuationReminderText`：前缀形式，或 `<system-reminder>` 里带续跑标记。
pub fn is_goal_continuation_reminder_text(text: &str) -> bool {
    let normalized = js_trim_start(text);
    normalized.starts_with(GOAL_REMINDER_PREFIX)
        || (normalized.starts_with("<system-reminder>")
            && normalized.contains(GOAL_CONTINUATION_MARKER))
}

/// TS `isESCodeGoalContinuationReminderMessage`：用户消息且带续跑标记（source/metadata.source/legacy 文本）。
pub fn is_goal_continuation_reminder_message(message: &Value) -> bool {
    is_user(message)
        && (opt_str(info(message), "source") == Some(GOAL_CONTINUATION_SOURCE)
            || info_metadata_source(message) == Some(GOAL_CONTINUATION_SOURCE)
            || is_goal_continuation_reminder_text(&text_from_parts(parts_of(message))))
}

/// TS `isESCodeGoalStateReminderText`：状态提示不是续跑边界（见迭代号计算）。
pub fn is_goal_state_reminder_text(text: &str) -> bool {
    let normalized = js_trim_start(text);
    normalized.starts_with("<system-reminder>") && normalized.contains(GOAL_STATE_MARKER)
}

/// TS `isESCodeGoalModelOnlyReminderMessage`：续跑提醒或状态提示，两者都只进模型上下文。
pub fn is_goal_model_only_reminder_message(message: &Value) -> bool {
    is_user(message)
        && (is_goal_continuation_reminder_message(message)
            || is_goal_state_reminder_text(&text_from_parts(parts_of(message))))
}

/// TS `isESCodeModelOnlySyntheticUserMessage`：子 agent 注入的 user 消息，或投影面本身不是用户输入。
pub fn is_model_only_synthetic_user_message(message: &Value) -> bool {
    let subagent = is_user(message)
        && (opt_str(info(message), "source") == Some("subagent_message")
            || info_metadata_source(message) == Some("subagent_message")
            || parts_of(message).iter().any(|part| {
                part.get("type").and_then(Value::as_str) == Some("text")
                    && metadata_source(metadata_map(part.get("metadata")))
                        == Some("subagent_message")
            }));
    let projection = policy::projection_policy(message);
    subagent || projection == Policy::ProviderContextOnly || projection == Policy::HiddenSynthetic
}

/// TS `isESCodeCompactSummaryMessage`：user 消息里带 `timelineStatus` 缺失的 compaction part。
pub fn is_compact_summary_message(message: &Value) -> bool {
    // compact summary 是压缩后的模型上下文，不是用户可见输入（timeline lifecycle part 会带 timelineStatus）。
    is_user(message)
        && parts_of(message).iter().any(|part| {
            part.get("type").and_then(Value::as_str) == Some("compaction")
                && opt_str(metadata_map(part.get("metadata")), "timelineStatus").is_none()
        })
}

/// TS `getESCodeUserVisibleMessages`：过滤 runtime 注入的模型上下文，返回消息克隆。
pub fn user_visible_messages(messages: &[Value]) -> Vec<Value> {
    // 这些都是 runtime 注入给模型的上下文，不是用户真实 query；可见投影必须过滤，避免快照/远程 workspace恢复
    // 时渲染成右侧用户气泡。
    messages
        .iter()
        .filter(|m| !is_model_only_synthetic_user_message(m) && !is_compact_summary_message(m))
        .cloned()
        .collect()
}

/// TS `getESCodeGoalIterationByAssistantMessageId`：assistant messageId → goal 迭代号。
pub fn goal_iteration_by_assistant_message_id(
    messages: &[Value],
    max_goal_iteration: Option<u32>,
    target: Option<&Value>,
) -> BTreeMap<String, u32> {
    let mut iterations = BTreeMap::new();
    // `options.target ?? null` + `if (!target) return new Map()`：没有目标就没有迭代号。
    let Some(target) = target.filter(|value| js_truthy(value)) else {
        return iterations;
    };
    let target_created_at = target.get("createdAt").and_then(Value::as_f64);
    // `target.status === "active" ? null : target.updatedAt`；字段缺失在 JS 里是 NaN 比较 = 不设界。
    let inactive_at = if target.get("status").and_then(Value::as_str) == Some("active") {
        None
    } else {
        target.get("updatedAt").and_then(Value::as_f64)
    };

    let mut current: u32 = 0;
    let mut pending_visible_goal_input = false;
    let mut sorted: Vec<&Value> = messages.iter().collect();
    sorted.sort_by(|left, right| compare_messages_by_created_time(left, right)); // JS `sort` 稳定

    for message in sorted {
        let role = opt_str(info(message), "role");
        if role == Some("user") {
            // 边界只来自 Continue/source=goal-continuation；`Current session goal state` 不推进迭代号。
            if is_goal_continuation_reminder_message(message) {
                current += 1;
                pending_visible_goal_input = false;
            } else if is_visible_real_goal_user_message(message, target) {
                // 可见 /goal 输入只用于旧快照缺 continuation 时补第一段边界，不能和 continuation 叠加。
                pending_visible_goal_input = true;
            }
            continue;
        }
        if role != Some("assistant") {
            continue;
        }

        let created_at = message_created_at(message);
        if let Some(inactive_at) = inactive_at {
            // 目标停止/完成后的追问仍在同一 session 内，不能继承旧 goal 轮次（否则被当成「第 N 次迭代」）。
            if created_at.is_some_and(|created| created > inactive_at) {
                continue;
            }
        }
        if pending_visible_goal_input {
            current += 1;
            pending_visible_goal_input = false;
        }
        // 旧会话缺 `/goal` 可见气泡或输入早于 target.createdAt：第一条 goal 后的 assistant 仍算第 1 轮。
        if current == 0
            && created_at
                .zip(target_created_at)
                .is_some_and(|(created, goal_created)| created >= goal_created)
        {
            current = 1;
        }
        if current > 0 {
            // assistant 桶不能推进到 verifier 尚未确认的下一轮；maxGoalIteration 按 verifier 上限收敛。
            let bounded = max_goal_iteration
                .filter(|max| *max > 0)
                .map_or(current, |max| current.min(max));
            if let Some(id) = message_id(message) {
                iterations.insert(id.to_owned(), bounded);
            }
        }
    }

    iterations
}

/// TS `getESCodeGoalActiveIterationCount`：当前应展示的 goal 迭代号（1 起，0 表示没有目标）。
pub fn goal_active_iteration_count(target_status: Option<&str>, timeline: Option<&[Value]>) -> u32 {
    // `if (!input.targetStatus) return 0`：缺省与空串都算「没有目标」。
    let Some(target_status) = target_status.filter(|status| !status.is_empty()) else {
        return 0;
    };
    let timeline = timeline.unwrap_or(&[]);
    // `latest.goalIteration ?? timeline.length`；非整数（schema 之外）退化成条数。
    let Some(latest) = timeline.last() else {
        return 1;
    };
    let latest_iteration = latest
        .get("goalIteration")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(timeline.len() as u32);
    let status = latest.get("status").and_then(Value::as_str);
    if status == Some("started") {
        return latest_iteration;
    }
    let passed = latest
        .get("verification")
        .and_then(|v| v.get("passed"))
        .and_then(Value::as_bool);
    if (status == Some("completed") && passed == Some(true)) || target_status == "complete" {
        return latest_iteration;
    }
    if target_status != "active" {
        // stop 先把 target 改成 paused、再把 verifier 收口成 cancelled；非 active 目标不自动续跑，不
        // 能把 cancelled/failed verifier 预投影成下一轮。
        return latest_iteration;
    }
    latest_iteration.saturating_add(1)
}

/// TS `resolveESCodeVisibleSessionTitle`：标题 → 首条真实用户输入 → objective → fallback。
pub fn visible_session_title(
    title: Option<&str>,
    messages: &[Value],
    target: Option<&Value>,
    fallback: Option<&str>,
) -> String {
    let normalized_title = js_trim(title.unwrap_or(""));
    if !normalized_title.is_empty()
        && !is_goal_continuation_reminder_text(normalized_title)
        && !is_goal_state_reminder_text(normalized_title)
    {
        return normalized_title.to_owned();
    }

    // 首条真实用户输入（跳过 goal model-only 提醒）的 parts 文本。
    let first_user = messages.iter().find(|message| {
        is_user(message)
            && policy::projection_policy(message) == Policy::RealUserInput
            && !is_goal_model_only_reminder_message(message)
    });
    let first_user_text = first_user.map_or(String::new(), |message| {
        js_trim(&text_from_parts(parts_of(message))).to_owned()
    });
    if !first_user_text.is_empty() {
        return truncate_utf16(&first_user_text);
    }

    // `input.target?.objective.trim() ?? ""`。
    let objective = target
        .and_then(|goal| goal.get("objective"))
        .and_then(Value::as_str);
    if let Some(objective) = objective.map(js_trim).filter(|text| !text.is_empty()) {
        return truncate_utf16(objective);
    }
    // `input.fallback ?? "New session"`：空串是「有值」，原样返回（与 `??` 一致）。
    fallback.unwrap_or("New session").to_owned()
}

/// TS `isVisibleRealGoalUserMessage`：真实用户输入且落在 `target.createdAt` 前 30 秒内（缺字段 = NaN 比较）。
fn is_visible_real_goal_user_message(message: &Value, target: &Value) -> bool {
    let (Some(created), Some(goal_created)) = (
        message_created_at(message),
        target.get("createdAt").and_then(Value::as_f64),
    ) else {
        return false;
    };
    is_user(message)
        && policy::projection_policy(message) == Policy::RealUserInput
        && created >= goal_created - 30_000.0
}

/// TS `compareESCodeMessagesByCreatedTime`：`time.created` 升序，相等时按 messageId 字节序
/// （`localeCompare` 替代，messageId 是 ASCII）；字段缺失在 JS 里是 NaN，V8 按 0 处理。
fn compare_messages_by_created_time(left: &Value, right: &Value) -> Ordering {
    let (Some(left_created), Some(right_created)) =
        (message_created_at(left), message_created_at(right))
    else {
        return Ordering::Equal;
    };
    match left_created.partial_cmp(&right_created) {
        Some(Ordering::Equal) => match (message_id(left), message_id(right)) {
            (Some(left_id), Some(right_id)) => left_id.cmp(right_id),
            _ => Ordering::Equal,
        },
        Some(ordering) => ordering,
        None => Ordering::Equal,
    }
}

fn message_created_at(message: &Value) -> Option<f64> {
    message.get("info")?.get("time")?.get("created")?.as_f64()
}
fn message_id(message: &Value) -> Option<&str> {
    message.get("info")?.get("messageId")?.as_str()
}

/// JS `String.prototype.slice(0, 80)`：按 **UTF-16 码元**截断，不是按字符。
fn truncate_utf16(value: &str) -> String {
    let mut units: Vec<u16> = value.encode_utf16().collect();
    if units.len() > 80 {
        units.truncate(80);
        // 截断点若落在高代理元上，JS 会留下孤立代理元；Rust 无法表示，少取一个码元。
        if units
            .last()
            .copied()
            .is_some_and(|unit| (0xd800..=0xdbff).contains(&unit))
        {
            units.pop();
        }
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试用 `#[rustfmt::skip]` 保持紧凑：rustfmt 会把长 fixture 折成多行、撑破 400 行上限。
    #[rustfmt::skip]
    fn json(raw: &str) -> Value { serde_json::from_str(raw).expect("json") }
    /// 一条消息：`role`/`messageId`/`time.created` + info 追加片段 + parts 片段。
    #[rustfmt::skip]
    fn msg(role: &str, id: &str, created: u64, extra: &str, parts: &str) -> Value {
        json(&format!("{{\"info\":{{\"role\":\"{role}\",\"messageId\":\"{id}\",\"time\":{{\"created\":{created}}}{extra}}},\"parts\":{parts}}}"))
    }

    #[test]
    #[rustfmt::skip]
    fn goal_reminder_texts() {
        // 前缀形式与 `<system-reminder>` + 续跑标记都算续跑提醒；状态提醒只算 model-only 提醒。
        for (text, reminder, state) in [
            ("<system-reminder source=\"goal-continuation\">x", true, false),
            ("<system-reminder>Continue working toward the active session goal.", true, false),
            ("<system-reminder>Current session goal state", false, true),
            ("\n  <system-reminder>Current session goal state", false, true),
        ] {
            assert_eq!(is_goal_continuation_reminder_text(text), reminder, "{text}");
            assert_eq!(is_goal_state_reminder_text(text), state, "{text}");
        }
        // 消息级：source / metadata.source 也算；assistant 一律不算。
        assert!(is_goal_continuation_reminder_message(&msg("user", "u1", 1, r#","source":"goal-continuation""#, "[]")));
        assert!(is_goal_continuation_reminder_message(&msg("user", "u2", 2, r#","metadata":{"source":"goal-continuation"}"#, "[]")));
    }

    #[test]
    #[rustfmt::skip]
    fn visible_messages_filter_injected_context() {
        // 子 agent、model-only、compact summary、legacy 通知都是注入上下文 → 全部过滤。
        for injected in [
            msg("user", "u1", 1, r#","metadata":{"source":"subagent_message"}"#, "[]"),
            msg("user", "u2", 2, r#","visibility":"model-only""#, "[]"),
            msg("user", "u3", 3, "", r#"[{"type":"compaction"}]"#),
            msg("user", "u4", 4, r#","synthetic":true"#, r#"[{"type":"text","text":"<task-notification>x"}]"#),
        ] {
            assert!(user_visible_messages(&[injected]).is_empty());
        }
        assert_eq!(user_visible_messages(&[msg("user", "u5", 5, "", r#"[{"type":"text","text":"hi"}]"#)]).len(), 1);
    }

    #[test]
    #[rustfmt::skip]
    fn goal_iterations_follow_boundaries() {
        let t = json(r#"{"status":"active","createdAt":0}"#);
        let continuation = r#","source":"goal-continuation""#;
        let messages = vec![
            msg("user", "u1", 1, continuation, "[]"),
            msg("assistant", "a1", 2, "", "[]"),
            msg("user", "u2", 3, continuation, "[]"),
            msg("assistant", "a2", 4, "", "[]"),
        ];
        let iterations = goal_iteration_by_assistant_message_id(&messages, None, Some(&t));
        assert_eq!(iterations.get("a1"), Some(&1));
        assert_eq!(iterations.get("a2"), Some(&2));
        assert_eq!(goal_iteration_by_assistant_message_id(&messages, Some(1), Some(&t)).get("a2"), Some(&1));
        assert!(goal_iteration_by_assistant_message_id(&messages, None, None).is_empty());
        // 非 active：updatedAt 之后的 assistant 不再继承旧轮次；active 目标不设界。
        let later = [msg("user", "u3", 1, continuation, "[]"), msg("assistant", "a3", 60, "", "[]")];
        let paused = json(r#"{"status":"paused","createdAt":0,"updatedAt":50}"#);
        assert_eq!(goal_iteration_by_assistant_message_id(&later, None, Some(&paused)).get("a3"), None);
        let active = json(r#"{"status":"active","createdAt":0,"updatedAt":50}"#);
        assert_eq!(goal_iteration_by_assistant_message_id(&later, None, Some(&active)).get("a3"), Some(&1));
    }

    #[test]
    #[rustfmt::skip]
    fn goal_iterations_take_visible_goal_input_as_first_segment() {
        let t = json(r#"{"status":"active","createdAt":100000}"#);
        let visible = r#","semantics":{"origin":"real_user"}"#;
        // 窗口外（createdAt - 30s 之前）的可见输入不算边界：没有 continuation 就没有轮次。
        assert!(goal_iteration_by_assistant_message_id(&[msg("user", "u0", 1_000, visible, "[]")], None, Some(&t)).is_empty());
        // 窗口内的可见 /goal 输入补第一段边界。
        let inside = [msg("user", "u1", 80_000, visible, "[]"), msg("assistant", "a1", 80_001, "", "[]")];
        assert_eq!(goal_iteration_by_assistant_message_id(&inside, None, Some(&t)).get("a1"), Some(&1));
    }

    #[test]
    #[rustfmt::skip]
    fn goal_active_iteration_count_uses_verifier_timeline() {
        let cancelled = json(r#"[{"status":"cancelled","goalIteration":2}]"#);
        let timeline = || cancelled.as_array().map(Vec::as_slice);
        assert_eq!(goal_active_iteration_count(None, None), 0);
        assert_eq!(goal_active_iteration_count(Some("active"), timeline()), 3);
        assert_eq!(goal_active_iteration_count(Some("paused"), timeline()), 2);
    }
    #[test]
    #[rustfmt::skip]
    fn visible_title_fallback_chain() {
        let t = json(r#"{"objective":"  objective text  "}"#);
        let first = msg("user", "u1", 1, r#","semantics":{"origin":"real_user"}"#, r#"[{"type":"text","text":"  first user text  "}]"#);
        // 标题 → 首条真实用户输入（trim）→ objective（trim）→ fallback（空串是「有值」，原样返回）。
        assert_eq!(visible_session_title(Some("  my title  "), std::slice::from_ref(&first), Some(&t), None), "my title");
        assert_eq!(visible_session_title(None, &[first], Some(&t), None), "first user text");
        assert_eq!(visible_session_title(None, &[], Some(&t), None), "objective text");
        assert_eq!(visible_session_title(None, &[], None, Some("fb")), "fb");
        assert_eq!(visible_session_title(None, &[], None, None), "New session");
        let emoji = json(&format!(r#"{{"objective":"{}"}}"#, "😀".repeat(41)));
        assert_eq!(visible_session_title(None, &[], Some(&emoji), None), "😀".repeat(40));
    }
}
