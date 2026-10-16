use super::map::classify_call_source;
use super::*;

#[test]
fn session_segments_are_sanitized_like_ts() {
    assert_eq!(sanitize_session_segment("abc-DEF_1"), "abc-DEF_1");
    assert_eq!(sanitize_session_segment("a b/c"), "a-b-c");
    assert_eq!(sanitize_session_segment("--x--"), "x");
    assert_eq!(sanitize_session_segment("///"), "");
    assert_eq!(sanitize_session_segment(&"a".repeat(100)).len(), 80);
}

#[test]
fn iso_dates_parse_like_date_parse() {
    assert_eq!(to_time(Some("1970-01-01T00:00:00.000Z")), 0);
    assert_eq!(to_time(Some("2026-10-15T08:00:00.123Z")), 1_792_051_200_123);
    assert_eq!(
        to_time(Some("2026-10-15T10:00:00+02:00")),
        1_792_051_200_000
    );
    assert_eq!(to_time(Some("nonsense")), 0);
    assert_eq!(to_time(None), 0);
}

#[test]
fn call_sources_follow_the_ts_precedence() {
    assert_eq!(
        classify_call_source(Some("main_turn"), Some("compact"))["kind"],
        "main"
    );
    assert_eq!(
        classify_call_source(None, Some("compact"))["kind"],
        "compact"
    );
    assert_eq!(classify_call_source(Some("x"), None)["kind"], "sidecar");
    assert_eq!(
        classify_call_source(None, Some("subagent"))["kind"],
        "subagent"
    );
    assert_eq!(classify_call_source(None, None)["kind"], "main");
}

#[tokio::test]
async fn delta_records_are_expanded_from_the_previous_request() {
    let dir = tempfile::tempdir().expect("dir");
    let lines = [
        json!({"type": "model_io", "sessionId": "t", "requestId": "r1", "startedAt": "2026-01-01T00:00:01.000Z",
            "request": {"messages": [{"role": "user", "content": "a"}, {"role": "assistant", "content": "b"}]}}),
        json!({"type": "model_io", "sessionId": "t", "requestId": "r2", "startedAt": "2026-01-01T00:00:02.000Z",
            "request": {"messagesKind": "delta", "messageOffset": 1, "messages": [{"role": "user", "content": "c"}]}}),
    ];
    let text = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    tokio::fs::write(dir.path().join("model-io-t.jsonl"), text)
        .await
        .expect("write");
    let result =
        read_model_trajectory(&[dir.path().to_path_buf()], "t", None, &|a, b| a.cmp(b)).await;
    let second = &result["records"][1]["request"]["messages"];
    assert_eq!(second.as_array().map(Vec::len), Some(2));
    assert_eq!(second[0]["parts"][0]["text"], "a");
    assert_eq!(second[1]["parts"][0]["text"], "c");
}
