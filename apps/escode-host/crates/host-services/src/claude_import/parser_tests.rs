use super::*;
use serde_json::json;

fn records() -> Vec<Value> {
    vec![
        json!({"type":"user","cwd":"/repo","timestamp":"2023-11-14T22:13:20Z","message":{"content":"第一条问题"}}),
        json!({"type":"assistant","timestamp":1_700_000_001_000_i64,"message":{"model":"claude-x","content":[{"type":"text","text":"答"}]}}),
        json!({"type":"assistant","message":{"content":[{"type":"text","text":"案"}]}}),
        json!({"type":"user","message":{"content":[{"type":"tool_result","content":"noise"}]}}),
        json!({"type":"user","timestamp":1_700_000_002_000_i64,"message":{"content":"第二条"}}),
        json!({"type":"assistant","message":{"model":"<synthetic>","content":"No response requested."}}),
    ]
}

#[test]
fn visible_messages_aggregate_fragmented_assistant_turns() {
    let parsed =
        parse_records(&records(), "/fallback", "s1", "/src/a.jsonl", None, None).expect("parsed");
    assert_eq!(parsed.workspace_path, "/repo");
    assert_eq!(parsed.title, "第一条问题");
    assert_eq!(parsed.messages.len(), 3, "{:?}", parsed.messages);
    assert_eq!(parsed.messages[0]["role"], "user");
    assert_eq!(parsed.messages[1]["role"], "assistant");
    assert_eq!(parsed.messages[1]["content"], "答案");
    assert_eq!(parsed.messages[1]["model"], "claude-x");
    assert_eq!(parsed.messages[2]["content"], "第二条");
    assert_eq!(parsed.created_at, 1_700_000_000_000);
    assert_eq!(parsed.updated_at, 1_700_000_002_000);
}

#[test]
fn empty_visible_history_is_an_error() {
    let error = parse_records(
        &[json!({"type":"assistant","message":{"model":"<synthetic>","content":"x"}})],
        "/w",
        "s1",
        "/src",
        None,
        None,
    )
    .expect_err("no visible messages");
    assert!(error.contains("没有可导入的可见消息"), "{error}");
}

#[test]
fn timestamps_accept_numbers_and_rfc3339() {
    assert_eq!(
        to_timestamp_ms(&json!("2023-11-14T22:13:20.123Z")),
        Some(1_700_000_000_123)
    );
    assert_eq!(
        to_timestamp_ms(&json!(1_700_000_000_i64)),
        Some(1_700_000_000_000)
    );
}
