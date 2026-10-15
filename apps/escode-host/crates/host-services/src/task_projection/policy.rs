//! TS `packages/shared/src/conversation-message-projection-policy.ts` 的 Rust 投影：一条
//! `{info, parts}` 消息属于哪个投影面（真实用户输入 / 可见 assistant / 仅 provider 上下文 /
//! 仅 timeline / 隐藏合成）。Host 的任务域快照读面（可见消息过滤、标题回退、状态派生）都以此为准。
//!
//! 差异登记（TS 语义见各函数注释）：
//! - `??` 判「无值」：serde_json 没有 `Undefined` 变体，键缺失即 undefined、`Value::Null` 即 null，
//!   两者对 `??` 都让位 → Rust 统一按「无值」处理；而 `info.summary !== undefined` 判的是
//!   **键存在**（显式 null 也算），用 `contains_key`，两者不可混用。
//! - TS `String(x)` 强转（`String(info.metadata?.["source"] ?? "")`）与 schema 外的非字符串字段
//!   （number/boolean）会产出 `"1"`/`"true"`；Rust 只认字符串，这类字段一律按「无值」让位。
//! - `messageId.localeCompare` 用 `str::cmp`（字节序）代替：messageId 是 ASCII，两者一致。
//! - 文本抽取两条路径**不可合并**：`text_from_parts` 只按 `type === "text"` 过滤，
//!   `projectable_text`（TS 私有 `textFromParts`）额外过滤 `ignored`。

use serde_json::{Map, Value};

/// TS `ConversationMessageProjectionPolicy`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    RealUserInput,
    VisibleAssistant,
    ProviderContextOnly,
    TimelineOnly,
    HiddenSynthetic,
}

/// TS 的 `Record<string, unknown>` 对象形态（`metadataRecord` 已滤掉非对象）。
pub(crate) type Record = Map<String, Value>;

// goal 续跑/状态提醒的标记；`visible.rs` 的判定共用这一份，避免两处漂移。
pub(crate) const GOAL_CONTINUATION_SOURCE: &str = "goal-continuation";
pub(crate) const GOAL_REMINDER_PREFIX: &str = "<system-reminder source=\"goal-continuation\">";
pub(crate) const GOAL_CONTINUATION_MARKER: &str =
    "Continue working toward the active session goal.";
pub(crate) const GOAL_STATE_MARKER: &str = "Current session goal state";

const MODEL_ONLY_VISIBILITY: &str = "model-only";
const FORK_SOURCE: &str = "fork";
const REWIND_MARKERS: [&str; 2] = ["Conversation rewind applied.", "Workspace rewind applied."];

/// TS `PROVIDER_CONTEXT_SYNTHETIC_SOURCES`：这些 source 的 user carrier 只进模型上下文（空白分隔的
/// 集合，免得 19 项 or-pattern 被 rustfmt 竖排成 19 行、撑破本文件的 400 行预算）。
const PROVIDER_CONTEXT_SYNTHETIC_SOURCES: &str = "\
agent_listing_delta agent_control_message background_task goal-continuation \
goal_completion_verification goal_state_change plugin_reference bot_topic_context \
queued_system_notification resume_goal_state resume_referenced_session_context rewind \
selection_side_chat subagent subagent_message target_continuation task_notification \
task_status todo_reminder";

/// TS `getConversationMessageProjectionPolicy`。判定顺序与 TS 逐行同序，短路点不可调换。
pub fn projection_policy(message: &Value) -> Policy {
    let info = message.get("info").and_then(Value::as_object);
    let parts = parts_of(message);
    let sem = info
        .and_then(|record| record.get("semantics"))
        .and_then(Value::as_object);

    // `info.summary !== undefined`：键存在即命中（显式 null 也算），不是 `??`。
    if opt_str(sem, "kind") == Some("compact_summary")
        || info.is_some_and(|record| record.contains_key("summary"))
    {
        return Policy::ProviderContextOnly;
    }

    // TS `if (semantics)` 是「真值对象」门槛；`Some(&Map)` 与真值对象同义。
    if sem.is_some() {
        if opt_str(sem, "kind") == Some("timeline_event") {
            return Policy::TimelineOnly;
        }
        if opt_str(sem, "origin") == Some("real_user")
            && !opt_flag(info, "synthetic")
            && opt_str(info, "visibility") != Some(MODEL_ONLY_VISIBILITY)
        {
            return Policy::RealUserInput;
        }
        if opt_str(info, "role") == Some("assistant")
            && opt_str(sem, "kind") == Some("assistant_response")
            && opt_str(sem, "uiVisibility") == Some("visible")
            && opt_str(sem, "transcriptVisibility") == Some("visible")
        {
            // 正常 assistant 同时会进入 provider context 和可见 transcript；
            // providerVisibility 不能抢先把它归成 model-only，否则晚订阅的 cold hydration 会丢正文。
            return Policy::VisibleAssistant;
        }
        if opt_str(sem, "providerVisibility") == Some("visible") {
            return Policy::ProviderContextOnly;
        }
        if opt_str(sem, "kind") == Some("fork_notice") {
            return Policy::TimelineOnly;
        }
        if opt_str(sem, "origin") == Some("agent_runtime")
            || opt_str(sem, "uiVisibility") == Some("hidden")
            || opt_str(sem, "transcriptVisibility") == Some("hidden")
        {
            return Policy::HiddenSynthetic;
        }
    }

    if opt_str(info, "visibility") == Some(MODEL_ONLY_VISIBILITY) || has_model_only_part(parts) {
        return Policy::ProviderContextOnly;
    }
    if is_timeline_only_message(info, sem, parts) {
        return Policy::TimelineOnly;
    }

    let source = message_source(info, sem, parts);
    if source == Some(FORK_SOURCE) {
        return Policy::TimelineOnly;
    }
    if source.is_some_and(is_provider_context_synthetic_source) {
        return Policy::ProviderContextOnly;
    }
    if has_legacy_system_reminder_context_text(parts) {
        return Policy::ProviderContextOnly;
    }

    let synthetic =
        opt_flag(info, "synthetic") || parts.iter().any(|p| opt_flag(p.as_object(), "synthetic"));
    if synthetic && has_legacy_notification_context_text(parts) {
        return Policy::ProviderContextOnly;
    }
    if synthetic {
        return Policy::HiddenSynthetic;
    }

    // 兜底：assistant 进可见面，其余（user/未知 role）按真实用户输入。
    if opt_str(info, "role") == Some("assistant") {
        Policy::VisibleAssistant
    } else {
        Policy::RealUserInput
    }
}

/// TS `PROVIDER_CONTEXT_SYNTHETIC_SOURCES` 的成员判定。
fn is_provider_context_synthetic_source(source: &str) -> bool {
    PROVIDER_CONTEXT_SYNTHETIC_SOURCES
        .split_whitespace()
        .any(|item| item == source)
}

/// TS `textFromESCodeMessageParts`（`escode-protocol-legacy-types.ts:526`）：`type === "text"` 的
/// part 取 `text` 连接，**不过滤** `ignored`（与内部 `projectable_text` 不同，别合并）。
/// 非字符串 `text` 在 JS 里经 `join` 变 ""（schema 之外），Rust 直接跳过，结果一致。
pub fn text_from_parts(parts: &[Value]) -> String {
    parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect()
}

/// TS 私有 `textFromParts`：同 `text_from_parts`，但过滤 `ignored === true` 的 part。
fn projectable_text(parts: &[Value]) -> String {
    parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter(|p| p.get("ignored") != Some(&Value::Bool(true)))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect()
}

/// TS `isTimelineOnlyMessage`；`sem` 复用已解析的 semantics 对象。
fn is_timeline_only_message(info: Option<&Record>, sem: Option<&Record>, parts: &[Value]) -> bool {
    if opt_str(sem, "kind") == Some("timeline_event") {
        return true;
    }
    let info_source = info_metadata_source(info);
    if opt_str(info, "source") == Some(FORK_SOURCE) || info_source == Some(FORK_SOURCE) {
        return true;
    }
    parts.iter().any(|part| {
        let record = part.as_object();
        let metadata = metadata_map(record.and_then(|map| map.get("metadata")));
        opt_str(record, "type") == Some("timeline")
            || has_session_fork_context(metadata)
            || (opt_str(record, "type") == Some("compaction")
                && (has_string_key(metadata, "timelineStatus")
                    || has_string_key(record, "summaryMessageId")))
    })
}

/// TS `hasModelOnlyPart`：part 标注为 model-only，或来自 goal 续跑。
fn has_model_only_part(parts: &[Value]) -> bool {
    parts.iter().any(|part| {
        let metadata = metadata_map(part.get("metadata"));
        opt_str(metadata, "visibility") == Some(MODEL_ONLY_VISIBILITY)
            || metadata_source(metadata) == Some(GOAL_CONTINUATION_SOURCE)
    })
}

/// TS `messageSource`：`info.source ?? metadata.source ?? semantics.source ?? 首个 part 的 metadata.source`。
/// `??` 只让位 null/undefined：空串算「有值」（只是后续判定都不命中）；part 兜底按 `find(Boolean)` 跳过空串。
fn message_source<'a>(
    info: Option<&'a Record>,
    sem: Option<&'a Record>,
    parts: &'a [Value],
) -> Option<&'a str> {
    info.and_then(|r| r.get("source"))
        .filter(|s| !s.is_null())
        .and_then(Value::as_str)
        .or_else(|| info_metadata_source(info))
        .or_else(|| sem.and_then(|r| r.get("source")).and_then(Value::as_str))
        .or_else(|| part_metadata_source(parts))
}

/// TS `stringValue(info.metadata?.source)`。
fn info_metadata_source(info: Option<&Record>) -> Option<&str> {
    metadata_source(metadata_map(info.and_then(|r| r.get("metadata"))))
}

/// TS `parts.map(p => stringValue(p.metadata?.source)).find(Boolean)`：首个非空 source。
fn part_metadata_source(parts: &[Value]) -> Option<&str> {
    parts
        .iter()
        .find_map(|p| metadata_source(metadata_map(p.get("metadata"))))
}

/// TS `hasLegacySystemReminderContextText`：legacy 快照没有 source 标记，只能靠文本前缀识别
/// goal 续跑/状态提醒与 rewind 通知（都是 runtime 注入的上下文，不是用户输入）。
fn has_legacy_system_reminder_context_text(parts: &[Value]) -> bool {
    let text = projectable_text(parts);
    let text = js_trim_start(&text);
    text.starts_with(GOAL_REMINDER_PREFIX)
        || (text.starts_with("<system-reminder>")
            && (text.contains(GOAL_CONTINUATION_MARKER) || text.contains(GOAL_STATE_MARKER)))
        || REWIND_MARKERS.iter().any(|marker| text.contains(marker))
}

/// TS `hasLegacyNotificationContextText`：legacy 的后台任务/子 agent 通知同样是注入上下文。
fn has_legacy_notification_context_text(parts: &[Value]) -> bool {
    let text = projectable_text(parts);
    let text = js_trim_start(&text);
    text.starts_with("<task-notification>") || text.starts_with("<subagent-notification>")
}

/// TS `hasSessionForkContext`：`metadata.forkContext` 是对象且 `kind === "session_fork"`。
fn has_session_fork_context(metadata: Option<&Record>) -> bool {
    let fork_context = metadata_map(metadata.and_then(|map| map.get("forkContext")));
    opt_str(fork_context, "kind") == Some("session_fork")
}

/// `message.parts ?? []`（非数组按空处理）。
pub(crate) fn parts_of(message: &Value) -> &[Value] {
    match message.get("parts").and_then(Value::as_array) {
        Some(parts) => parts.as_slice(),
        None => &[],
    }
}

/// TS `metadataRecord`：只有普通对象成立（数组/标量/null/false 一律 undefined）。
pub(crate) fn metadata_map(value: Option<&Value>) -> Option<&Record> {
    value.and_then(Value::as_object)
}

/// TS `metadataRecord(x)?.source`：非字符串或空串 → 无值（等价 `stringValue`）。
pub(crate) fn metadata_source(metadata: Option<&Record>) -> Option<&str> {
    let raw = metadata
        .and_then(|map| map.get("source"))
        .and_then(Value::as_str);
    raw.filter(|text| !text.is_empty())
}

/// `record?.key`，且只接受字符串值（非字符串按 undefined；`String(...)` 强转差异见文件头）。
pub(crate) fn opt_str<'a>(record: Option<&'a Record>, key: &str) -> Option<&'a str> {
    record.and_then(|r| r.get(key)).and_then(Value::as_str)
}

/// `record?.key === true`（只认布尔 true；undefined/false/其他类型都不算）。
fn opt_flag(record: Option<&Record>, key: &str) -> bool {
    record.is_some_and(|r| r.get(key).and_then(Value::as_bool) == Some(true))
}

/// `typeof record?.key === "string"`（空串也算 string，与 `stringValue` 的取舍不同）。
fn has_string_key(record: Option<&Record>, key: &str) -> bool {
    record
        .and_then(|r| r.get(key))
        .is_some_and(Value::is_string)
}

/// JS 真值：`[]`/`{}` 为真，null/false/0/"" 为假（`if (target)`、`if (lastError)` 等依赖它）。
pub(crate) fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// JS `trim` 的空白集合。不用 `char::is_whitespace`：它是 Unicode `White_Space`，与 JS 只差
/// 两个码点（多算 NEL U+0085、少算 BOM U+FEFF），这里显式校正。
fn js_ws(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

/// JS `String.prototype.trimStart()`（含 BOM，见 `js_ws`）。
pub(crate) fn js_trim_start(text: &str) -> &str {
    text.trim_start_matches(js_ws)
}

/// JS `String.prototype.trim()`。
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(js_ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 表格里的短别名：完整变体名会让断言行超宽。
    const RU: Policy = Policy::RealUserInput;
    const VA: Policy = Policy::VisibleAssistant;
    const PC: Policy = Policy::ProviderContextOnly;
    const TL: Policy = Policy::TimelineOnly;
    const HS: Policy = Policy::HiddenSynthetic;

    /// info/parts 用 JSON 片段而不是 `json!`：宏的多行展开会吃掉 400 行预算。
    fn policy(info: &str, parts: &str) -> Policy {
        let raw = format!("{{\"info\":{info},\"parts\":{parts}}}");
        projection_policy(&serde_json::from_str(&raw).expect("message json"))
    }

    /// `info`（`"[]"` = 无 parts）与带 parts 的判定表。`#[rustfmt::skip]` 保一行一例：否则
    /// rustfmt 会把超 60 字符的 tuple 折成 4 行、撑破本文件的 400 行预算。
    #[rustfmt::skip]
    const CASES: &[(&str, &str, Policy)] = &[
        // 无 parts：只看 info。
        (r#"{}"#, "[]", RU),
        (r#"{"role":"user"}"#, "[]", RU),
        (r#"{"role":"assistant"}"#, "[]", VA),
        (r#"{"source":"fork"}"#, "[]", TL),
        (r#"{"metadata":{"source":"fork"}}"#, "[]", TL),
        (r#"{"synthetic":true}"#, "[]", HS),
        // `info.summary !== undefined` 判的是键存在：显式 null 也算存在（不是 `??`）。
        (r#"{"role":"user","summary":null}"#, "[]", PC),
        (r#"{"role":"user","metadata":{"source":"subagent_message"}}"#, "[]", PC),
        (r#"{"role":"user","semantics":{"origin":"real_user"}}"#, "[]", RU),
        // real_user 还要求 synthetic 未标记、visibility 非 model-only，否则继续往下判定。
        (r#"{"visibility":"model-only","semantics":{"origin":"real_user"}}"#, "[]", PC),
        (r#"{"synthetic":true,"semantics":{"origin":"real_user"}}"#, "[]", HS),
        (r#"{"semantics":{"kind":"timeline_event"}}"#, "[]", TL),
        (r#"{"semantics":{"kind":"fork_notice"}}"#, "[]", TL),
        (r#"{"semantics":{"origin":"agent_runtime"}}"#, "[]", HS),
        (r#"{"semantics":{"transcriptVisibility":"hidden"}}"#, "[]", HS),
        (r#"{"role":"assistant","semantics":{"kind":"compact_summary"}}"#, "[]", PC),
        // 可见 assistant 不能因 providerVisibility=visible 被归成 model-only（cold hydration 丢正文）；
        // 没有可见 transcript 的 provider context 仍是 model-only。
        (r#"{"role":"assistant","semantics":{"kind":"assistant_response","uiVisibility":"visible",
            "transcriptVisibility":"visible","providerVisibility":"visible"}}"#, "[]", VA),
        (r#"{"role":"assistant","semantics":{"kind":"assistant_response",
            "uiVisibility":"hidden","providerVisibility":"visible"}}"#, "[]", PC),
        // 带 parts：timeline 部件、fork 上下文、goal 续跑、legacy 前缀与忽略标记。
        (r#"{"role":"assistant"}"#, r#"[{"type":"timeline"}]"#, TL),
        (r#"{}"#, r#"[{"metadata":{"forkContext":{"kind":"session_fork"}}}]"#, TL),
        (r#"{}"#, r#"[{"type":"text","metadata":{"source":"goal-continuation"}}]"#, PC),
        (r#"{}"#, r#"[{"type":"text","synthetic":true}]"#, HS),
        (r#"{}"#, r#"[{"type":"text","text":"<task-notification>x"}]"#, RU),
        (r#"{"synthetic":true}"#, r#"[{"type":"text","text":"<task-notification>x"}]"#, PC),
        // 带 timelineStatus 的 compaction 是 timeline lifecycle（timeline-only）；缺该字段才是 compact summary。
        (r#"{}"#, r#"[{"type":"compaction","metadata":{"timelineStatus":"s"}}]"#, TL),
        // ignored 的文本不参与 legacy 提醒判定（内部 textFromParts 过滤它）。
        (r#"{}"#, r#"[{"type":"text","text":"<task-notification>","ignored":true}]"#, RU),
        (r#"{}"#, r#"[{"type":"text","text":" <system-reminder>Current session goal state"}]"#, PC),
        (r#"{}"#, r#"[{"type":"text","text":"<system-reminder source=\"goal-continuation\">x"}]"#,
            PC),
        (r#"{"role":"assistant","metadata":{"source":"resume_goal_state"}}"#, "[]", PC),
        // part 级 source 同样经 `messageSource` 命中 provider context 集合。
        (r#"{}"#, r#"[{"metadata":{"source":"subagent_message"}}]"#, PC),
    ];

    #[test]
    fn projection_policy_cases() {
        for (info, parts, expected) in CASES {
            assert_eq!(policy(info, parts), *expected, "{info} / {parts}");
        }
    }

    #[test]
    fn text_extraction_paths_differ_on_ignored() {
        let raw = r#"[{"type":"text","text":"a"},{"type":"text","text":"b","ignored":true}]"#;
        let parts: Vec<Value> = serde_json::from_str(raw).expect("parts json");
        // 公开抽取不吞 ignored（对齐 TS textFromESCodeMessageParts）；内部抽取过滤它。
        assert_eq!(text_from_parts(&parts), "ab");
        assert_eq!(projectable_text(&parts), "a");
    }

    #[test]
    fn js_helpers_match_javascript_edge_cases() {
        let truthy = |raw: &str| js_truthy(&serde_json::from_str::<Value>(raw).expect("json"));
        assert!(truthy("{}") && truthy("[]") && !truthy("null") && !truthy("0") && !truthy("\"\""));
        // BOM 算空白；U+0085 在 JS 里不是空白 → 不 trim；U+3000 是 → trim。
        assert_eq!(js_trim_start("\u{feff}  x"), "x");
        assert_eq!(js_trim_start("\u{85}x"), "\u{85}x");
        assert_eq!(js_trim_start("\u{3000}x"), "x");
    }
}
