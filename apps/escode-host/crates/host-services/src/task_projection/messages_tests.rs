use super::{coalesce_consecutive_assistants, map_message, reasoning_from_parts, text_from_parts};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn tool_part(state: Value) -> Value {
    json!({"type": "tool", "callId": "call-1", "tool": "Bash", "state": state})
}

#[test]
fn text_and_reasoning_join_by_part_type() {
    let parts = json!([
        {"type": "step-start"},
        {"type": "text", "text": "hello "},
        {"type": "reasoning", "text": "think-1"},
        {"type": "text", "text": "world"},
        {"type": "reasoning", "text": "think-2"}
    ]);
    let parts = match parts.as_array() {
        Some(parts) => parts.clone(),
        None => Vec::new(),
    };
    assert_eq!(text_from_parts(&parts), "hello world");
    assert_eq!(
        reasoning_from_parts(&parts),
        Some("think-1think-2".to_owned())
    );
    // 没有 reasoning part 时为空（TS 里 undefined）。
    assert_eq!(
        reasoning_from_parts(&[json!({"type": "text", "text": "x"})]),
        None
    );
    assert_eq!(text_from_parts(&[]), "");
}

#[test]
fn tool_status_mapping_covers_completed_error_and_running() {
    let completed = super::map_tool_part(
        &tool_part(
            json!({"status": "completed", "title": "List files", "input": {"command": "ls"},
            "output": "ok", "metadata": {"taskId": "t1"}}),
        ),
        None,
    );
    assert_eq!(completed.get("status"), Some(&json!("completed")));
    assert_eq!(completed.get("title"), Some(&json!("List files")));
    assert_eq!(completed.get("toolName"), Some(&json!("Bash")));
    assert_eq!(completed.get("output"), Some(&json!("ok")));
    // metadata 才是 raw 的载体，toolCallId 挂在 raw 顶层。
    assert_eq!(
        completed.get("raw").and_then(|raw| raw.get("taskId")),
        Some(&json!("t1"))
    );
    assert_eq!(
        completed.get("raw").and_then(|raw| raw.get("toolCallId")),
        Some(&json!("call-1"))
    );

    // failed notification 必须覆盖 snapshot 的 completed，并带出 notification error。
    let mut failed_notifications = BTreeMap::new();
    failed_notifications.insert(
        "call-1".to_owned(),
        json!({"status": "failed", "error": "boom"}),
    );
    let failed = super::map_tool_part(
        &tool_part(json!({"status": "completed", "title": "List", "input": {}, "output": "ack"})),
        Some(&failed_notifications),
    );
    assert_eq!(failed.get("status"), Some(&json!("failed")));
    assert_eq!(failed.get("error"), Some(&json!("boom")));
    assert_eq!(failed.get("output"), Some(&json!("ack")));

    // stopped/killed 只表示被停止，不能覆盖 completed。
    let mut stopped_notifications = BTreeMap::new();
    stopped_notifications.insert("call-1".to_owned(), json!({"status": "killed"}));
    let stopped = super::map_tool_part(
        &tool_part(json!({"status": "completed", "title": "t", "input": {}, "output": "o"})),
        Some(&stopped_notifications),
    );
    assert_eq!(stopped.get("status"), Some(&json!("completed")));
    assert_eq!(stopped.get("error"), None);

    let error = super::map_tool_part(
        &tool_part(json!({"status": "error", "input": {}, "error": "nope"})),
        None,
    );
    assert_eq!(error.get("status"), Some(&json!("failed")));
    // error 分支的 title 取工具名，不是 state.title。
    assert_eq!(error.get("title"), Some(&json!("Bash")));
    assert_eq!(error.get("error"), Some(&json!("nope")));

    let running = super::map_tool_part(
        &tool_part(
            json!({"status": "running", "title": "Running", "input": {"a": 1}, "startedAt": 5}),
        ),
        None,
    );
    assert_eq!(running.get("status"), None);
    assert_eq!(running.get("title"), Some(&json!("Running")));
    let bare = super::map_tool_part(&tool_part(json!({"status": "running", "input": {}})), None);
    assert_eq!(bare.get("title"), Some(&json!("Bash")));
}

#[test]
fn assistant_message_keeps_only_conditional_keys_with_values() {
    // 无 model、无 completed 时间：model/durationMs 等键必须缺席。
    let message = json!({"info": {"messageId": "a1", "role": "assistant", "time": {"created": 1000}},
        "parts": [{"type": "text", "text": "hi"}, {"type": "reasoning", "text": "think"}]});
    let mapped = map_message(&message, None, None);
    assert_eq!(mapped.get("id"), Some(&json!("a1")));
    assert_eq!(mapped.get("role"), Some(&json!("assistant")));
    assert_eq!(mapped.get("content"), Some(&json!("hi")));
    assert_eq!(mapped.get("timestamp"), Some(&json!(1000)));
    assert_eq!(mapped.get("thought"), Some(&json!("think")));
    assert_eq!(
        mapped.get("parts"),
        Some(
            &json!([{"type": "content", "content": "hi"}, {"type": "thought", "content": "think"}])
        )
    );
    for key in [
        "model",
        "syntheticTimeline",
        "attachments",
        "goalIteration",
        "durationMs",
        "tools",
    ] {
        assert!(mapped.get(key).is_none(), "{key} should stay absent");
    }

    // completed 时间非 0 → durationMs；completed === 0 视为无值；goalIteration 0 也省略。
    let timed = json!({"info": {"messageId": "a2", "role": "assistant",
        "time": {"created": 1000, "completed": 1500}, "model": {"providerId": "glm", "modelId": "glm-4"}},
        "parts": []});
    let mapped = map_message(&timed, Some(2), None);
    assert_eq!(mapped.get("durationMs"), Some(&json!(500)));
    assert_eq!(mapped.get("goalIteration"), Some(&json!(2)));
    assert_eq!(mapped.get("model"), Some(&json!("glm/glm-4")));
    // 空 parts：thought/tools/parts 都不落键。
    for key in ["thought", "tools", "parts"] {
        assert!(mapped.get(key).is_none(), "{key} should stay absent");
    }
    let zero = json!({"info": {"messageId": "a3", "role": "assistant",
        "time": {"created": 10, "completed": 0}}, "parts": []});
    let mapped = map_message(&zero, Some(0), None);
    assert_eq!(mapped.get("durationMs"), None);
    assert_eq!(mapped.get("goalIteration"), None);
}

#[test]
fn user_message_projects_attachments_without_assistant_keys() {
    let message = json!({"info": {"messageId": "u1", "role": "user", "time": {"created": 1}},
    "parts": [
        {"type": "text", "text": "look"},
        {"type": "file", "url": "data:image/png;base64,QQ==", "mime": "image/png",
            "filename": "a.png", "metadata": {"sizeBytes": 2}}
    ]});
    let mapped = map_message(&message, Some(3), None);
    assert_eq!(mapped.get("content"), Some(&json!("look")));
    assert_eq!(
        mapped.get("attachments"),
        Some(
            &json!([{"kind": "image", "filename": "a.png", "mimeType": "image/png",
            "sizeBytes": 2, "dataBase64": "QQ=="}])
        )
    );
    // user 角色没有 assistant 区（即使传了 goalIteration 也不落键）。
    for key in ["thought", "tools", "parts", "durationMs", "goalIteration"] {
        assert!(mapped.get(key).is_none(), "{key} should stay absent");
    }
}

#[test]
fn coalesce_merges_same_turn_assistants_and_retags_turns() {
    let merged = coalesce_consecutive_assistants(vec![
        json!({"role": "user", "content": "q", "timestamp": 0, "turnIndex": 0}),
        json!({"id": "a1", "role": "assistant", "content": "one", "timestamp": 10,
            "durationMs": 5, "tools": [{"toolName": "Bash"}],
            "parts": [{"type": "content", "content": "one"}, {"type": "tool-call", "toolIndex": 0}]}),
        json!({"id": "a2", "role": "assistant", "content": "two", "timestamp": 20,
            "durationMs": 5, "model": "glm/x", "tools": [{"toolName": "Read"}],
            "parts": [{"type": "tool-call", "toolIndex": 0}, {"type": "content", "content": "two"}]}),
        json!({"role": "user", "content": "next", "timestamp": 30, "turnIndex": 1}),
        json!({"id": "a3", "role": "assistant", "content": "three", "timestamp": 40, "turnIndex": 1}),
    ]);
    // 四段：user / 合并后的 a1+a2 / user(next) / a3（user 是合并边界，不能跨过去）。
    assert_eq!(merged.len(), 4);
    let first = &merged[1];
    assert_eq!(first.get("content"), Some(&json!("onetwo")));
    assert_eq!(first.get("model"), Some(&json!("glm/x")));
    // durationMs = Math.max(next.timestamp + next.durationMs - first.timestamp, 0)
    assert_eq!(first.get("durationMs"), Some(&json!(15)));
    assert_eq!(first.get("mergedMessageIds"), Some(&json!(["a1", "a2"])));
    // toolIndex 按 first.tools 长度平移。
    assert_eq!(
        first.get("parts"),
        Some(&json!([
            {"type": "content", "content": "one"},
            {"type": "tool-call", "toolIndex": 0},
            {"type": "tool-call", "toolIndex": 1},
            {"type": "content", "content": "two"}
        ]))
    );
    assert_eq!(
        first
            .get("tools")
            .and_then(|tools| tools.as_array())
            .map(Vec::len),
        Some(2)
    );
    // 首个消息是 user（turn 0）；合并后的 assistant 仍属 turn 0，之后切到 turn 1。
    assert_eq!(first.get("turnIndex"), Some(&json!(0)));
    assert_eq!(merged[2].get("turnIndex"), Some(&json!(1)));
    assert_eq!(merged[3].get("turnIndex"), Some(&json!(1)));

    // 首个消息就是 assistant：turn 从 -1 起，取 0。
    let leading = coalesce_consecutive_assistants(vec![
        json!({"role": "assistant", "content": "x", "timestamp": 0}),
    ]);
    assert_eq!(leading[0].get("turnIndex"), Some(&json!(0)));
}

#[test]
fn coalesce_keeps_boundaries_for_synthetic_timeline_and_goal_iteration() {
    let synthetic = coalesce_consecutive_assistants(vec![
        json!({"id": "a1", "role": "assistant", "content": "one", "timestamp": 1}),
        json!({"id": "a2", "role": "assistant", "content": "two", "timestamp": 2,
            "syntheticTimeline": {"type": "context_compaction"}}),
    ]);
    assert_eq!(synthetic.len(), 2);
    assert_eq!(synthetic[0].get("content"), Some(&json!("one")));
    assert_eq!(synthetic[1].get("content"), Some(&json!("two")));

    let goal = coalesce_consecutive_assistants(vec![
        json!({"id": "a1", "role": "assistant", "content": "one", "timestamp": 1, "goalIteration": 1}),
        json!({"id": "a2", "role": "assistant", "content": "two", "timestamp": 2, "goalIteration": 2}),
    ]);
    assert_eq!(goal.len(), 2);

    // 非 assistant 相邻（user 夹在中间）不合并。
    let split = coalesce_consecutive_assistants(vec![
        json!({"id": "a1", "role": "assistant", "content": "one", "timestamp": 1}),
        json!({"role": "user", "content": "q", "timestamp": 2}),
        json!({"id": "a2", "role": "assistant", "content": "two", "timestamp": 3}),
    ]);
    assert_eq!(split.len(), 3);
    assert_eq!(split[0].get("content"), Some(&json!("one")));
}
