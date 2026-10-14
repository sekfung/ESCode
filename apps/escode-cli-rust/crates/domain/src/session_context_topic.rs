//! ReadSessionContext 的 topic 策略（v3.15.1，TS session-context/topic-archive.ts 与 topic-resource.ts）。

use serde_json::{Value, json};

use crate::session_context_lite::{DEFAULT_MAX_TOKENS, Input, MAX_TOKENS};
use crate::session_context_text::len16;

/// TS `attachment` 子对象（strict）：messageId 1-256，inputId 可选 1-256，index 非负整数。
pub(crate) fn parse_attachment(value: &Value) -> Result<Value, String> {
    let record = value.as_object().ok_or("attachment must be an object")?;
    if let Some(key) = record
        .keys()
        .find(|k| !matches!(k.as_str(), "inputId" | "messageId" | "index"))
    {
        return Err(format!("Unrecognized key: {key}"));
    }
    let bounded = |key: &str, required: bool| match record.get(key) {
        None if !required => Ok(()),
        Some(Value::String(s)) if (1..=256).contains(&len16(s)) => Ok(()),
        _ => Err(format!("attachment.{key} must be 1-256 characters")),
    };
    bounded("messageId", true)?;
    bounded("inputId", false)?;
    if record.get("index").is_some_and(|i| i.as_u64().is_none()) {
        return Err("attachment.index must be a non-negative integer".into());
    }
    Ok(value.clone())
}

/// TS `readTopicArchive` 在会话里没有可信 botGroupSource 时的结果（Rust runtime 不承载机器人
/// 话题输入，见 docs/specs/rust-v3.15.1-drift.md）：预算校验同 TS，返回 not_found 与空记录。
pub fn topic_archive_output(input: &Input) -> Result<Value, String> {
    let budget = input
        .max_tokens
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .min(MAX_TOKENS);
    if budget < 256 {
        return Err("Topic retrieval budget must be at least 256 tokens".into());
    }
    Ok(json!({
        "status": "not_found", "sessionId": input.session_id, "strategy": "topic",
        "query": input.query, "source": "local",
        "content": json!({
            "coverage": "Previously archived messages only; unread history is not available.",
            "records": [],
        })
        .to_string(),
        "messageCount": 0, "selectedMessageCount": 0, "truncated": false, "references": [],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_strategy_input_follows_ts_schema() {
        let parse = |v: Value| Input::parse(&v);
        let topic = parse(json!({"query": "q", "strategy": "topic", "cursor": 3})).unwrap();
        assert_eq!(topic.session_id, "current");
        assert_eq!(topic.cursor, Some(3));
        assert_eq!(
            parse(json!({"query": "q"})).unwrap_err(),
            "Non-topic strategies require an explicit session id"
        );
        assert_eq!(
            parse(json!({"sessionId": "sess_a", "query": "q", "attachment": {"messageId": "m"}}))
                .unwrap_err(),
            "Attachments require topic strategy"
        );
        assert!(
            parse(json!({"query": "q", "strategy": "topic", "attachment": {"messageId": ""}}))
                .is_err()
        );
        assert!(
            parse(
                json!({"query": "q", "strategy": "topic", "attachment": {"messageId": "m", "x": 1}})
            )
            .is_err()
        );
        let output = topic_archive_output(&topic).unwrap();
        assert_eq!(output["status"], "not_found");
        assert_eq!(
            output["content"],
            r#"{"coverage":"Previously archived messages only; unread history is not available.","records":[]}"#
        );
        let small = parse(json!({"query": "q", "strategy": "topic", "maxTokens": 100})).unwrap();
        assert!(topic_archive_output(&small).is_err());
    }
}
