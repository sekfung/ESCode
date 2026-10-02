//! model_io.rs 的单测（从内联 `mod tests` 挪出，保持文件在 400 行以内）。
use super::*;

fn record(messages: usize, error: bool) -> Value {
    let msgs: Vec<Value> = (0..messages)
        .map(|i| json!({"role": "user", "content": format!("m{i}")}))
        .collect();
    let mut record = json!({
        "type": "model_io",
        "request": {"messages": msgs, "body": {"messages": msgs, "model": "x"}},
    });
    if error {
        record["error"] = json!({"name": "e", "message": "boom"});
    }
    record
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn delta_after_baseline_and_tail_for_long_history() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), true, false, Some("s/1"), record(3, false)).unwrap();
    write(dir.path(), true, false, Some("s/1"), record(5, false)).unwrap();
    let path = dir.path().join("model-io-s-1.jsonl");
    let rows = lines(&path);
    assert_eq!(rows[0]["request"]["messagesKind"], "full");
    assert_eq!(rows[1]["request"]["messagesKind"], "delta");
    assert_eq!(rows[1]["request"]["messageOffset"], 3);
    assert_eq!(rows[1]["request"]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(rows[1]["request"]["bodyMessagesKind"], "delta");
    // 历史被改写（首条变了）→ 不能 delta，回到 baseline。
    let mut changed = record(6, false);
    changed["request"]["messages"][0]["content"] = "edited".into();
    write(dir.path(), true, false, Some("s/1"), changed).unwrap();
    assert_eq!(lines(&path)[2]["request"]["messagesKind"], "full");
    // 新 session 超长历史 → tail。
    write(dir.path(), true, false, Some("t"), record(300, false)).unwrap();
    let tail = &lines(&dir.path().join("model-io-t.jsonl"))[0]["request"];
    assert_eq!(tail["messagesKind"], "tail");
    assert_eq!(tail["messageOffset"], 300 - MAX_DEBUG_BASELINE_MESSAGES);
}

#[test]
fn production_trims_body_messages_except_on_error_and_rotates() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), false, false, Some("a"), record(2, false)).unwrap();
    let ok = &lines(&dir.path().join("model-io-a.jsonl"))[0]["request"];
    assert!(ok["body"].get("messages").is_none());
    assert_eq!(ok["body"]["model"], "x");
    write(dir.path(), false, false, Some("a"), record(3, true)).unwrap();
    let failed = &lines(&dir.path().join("model-io-a.jsonl"))[1]["request"];
    assert_eq!(failed["bodyMessagesKind"], "full");
    assert_eq!(failed["body"]["messages"].as_array().unwrap().len(), 3);
    for session in ["b", "c", "d"] {
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(dir.path(), false, false, Some(session), record(1, false)).unwrap();
    }
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["model-io-b.jsonl", "model-io-c.jsonl", "model-io-d.jsonl"]
    );
}

#[test]
fn full_retention_skips_compaction() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), false, true, None, record(2, false)).unwrap();
    write(dir.path(), false, true, None, record(4, false)).unwrap();
    let rows = lines(&dir.path().join("model-io-no-session.jsonl"));
    assert!(rows[1]["request"].get("messagesKind").is_none());
    assert_eq!(rows[1]["request"]["messages"].as_array().unwrap().len(), 4);
    assert_eq!(
        rows[1]["request"]["body"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn request_body_skips_messages_only_when_asked() {
    let body = br#"{"model":"m","messages":[{"role":"user","content":"x"}],"stream":true}"#;
    assert_eq!(
        request_body(body, false),
        Some(json!({"model": "m", "stream": true}))
    );
    assert_eq!(
        request_body(body, true).unwrap()["messages"][0]["content"],
        "x"
    );
}

#[test]
fn projection_matches_ts_model_input_message() {
    let assistant = json!({
        "role": "assistant", "content": "hi", "reasoning_content": "think",
        "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "Read", "arguments": "{\"p\":1}"}}],
        "_zcode_origin": {"provider": "p"},
    });
    assert_eq!(
        project_message(&assistant),
        json!({
            "role": "assistant",
            "content": [{"type": "reasoning", "text": "think"}, {"type": "text", "text": "hi"}],
            "toolCalls": [{"id": "c1", "name": "Read", "input": {"p": 1}}],
        })
    );
    let tool = json!({"role": "tool", "tool_call_id": "c1", "content": "ok", "_zcode_tool_failed": true, "_zcode_tool_name": "Read"});
    assert_eq!(
        project_message(&tool),
        json!({"role": "tool", "content": "ok", "toolCallId": "c1", "toolName": "Read", "isError": true})
    );
    assert_eq!(file_segment("a b/c..d--"), "a-b-c-d");
}
