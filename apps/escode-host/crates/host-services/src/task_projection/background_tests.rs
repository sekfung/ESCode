//! `background`（通知）与 `background_controls`（控制项）的语义边界用例。
//! 单独成文件以守住主文件 400 行上限。

use serde_json::{Value, json};

use super::super::background_controls::{parse_control_items, parse_iso_instant_ms};
use super::{
    attach_notification_to_raw, collect_notifications_by_tool_use_id,
    notification_tool_update_status,
};

fn user_message(text: &str) -> Value {
    json!({
        "info": {"role": "user", "messageId": "m1", "time": {"created": 1}},
        "parts": [{"type": "text", "text": text}],
    })
}

#[test]
fn notification_text_collects_all_tags_and_decodes_entities() {
    let text = "<task-notification>\n<tool-use-id>call_1</tool-use-id>\n\
                <status>failed</status>\n<summary>a &amp; b &lt;c&gt;</summary>\n\
                <error>boom</error>\n</task-notification>";
    let collected = collect_notifications_by_tool_use_id(&[user_message(text)]);
    let notification = collected.get("call_1").expect("notification");
    assert_eq!(notification["status"], json!("failed"));
    assert_eq!(notification["summary"], json!("a & b <c>"));
    assert_eq!(notification["error"], json!("boom"));
    // 无 tool-use-id / 无前缀 / 非 user 消息都不登记。
    assert!(
        collect_notifications_by_tool_use_id(&[user_message("<task-notification>")]).is_empty()
    );
    assert!(collect_notifications_by_tool_use_id(&[user_message("plain")]).is_empty());
    assert!(
        collect_notifications_by_tool_use_id(&[json!({
            "info": {"role": "assistant"},
            "parts": [{"type": "text", "text": "<task-notification><tool-use-id>x</tool-use-id>"}],
        })])
        .is_empty()
    );
}

#[test]
fn notification_tool_status_maps_killed_and_lost() {
    assert_eq!(notification_tool_update_status("failed"), Some("failed"));
    assert_eq!(notification_tool_update_status("lost"), Some("failed"));
    assert_eq!(notification_tool_update_status("stopped"), Some("stopped"));
    assert_eq!(notification_tool_update_status("killed"), Some("stopped"));
    assert_eq!(
        notification_tool_update_status("completed"),
        Some("completed")
    );
    // 未知状态折成 completed（TS 的兜底 return "completed"）。
    assert_eq!(
        notification_tool_update_status("whatever"),
        Some("completed")
    );
}

#[test]
fn attach_notification_merges_into_meta_escode() {
    let raw = json!({"toolCallId": "call_1", "_meta": {"other": 1, "escode": {"keep": true}}});
    let attached = attach_notification_to_raw(raw, Some(&json!({"status": "failed"})));
    assert_eq!(attached["toolCallId"], json!("call_1"));
    assert_eq!(attached["_meta"]["other"], json!(1));
    assert_eq!(attached["_meta"]["escode"]["keep"], json!(true));
    assert_eq!(
        attached["_meta"]["escode"]["taskNotification"],
        json!({"status": "failed"})
    );
    // 无通知时原样返回（不建空 `_meta`）。
    assert_eq!(
        attach_notification_to_raw(json!({"a": 1}), None),
        json!({"a": 1})
    );
    // 非对象 raw 按空记录处理，只补 `_meta`。
    let wrapped = attach_notification_to_raw(Value::Null, Some(&json!({"status": "lost"})));
    assert_eq!(
        wrapped["_meta"]["escode"]["taskNotification"],
        json!({"status": "lost"})
    );
}

#[test]
fn control_items_resolve_kind_command_and_status_aliases() {
    let jobs = parse_control_items(&[
        json!({"toolName": "Task", "description": "跑一遍迁移", "status": "in_progress",
               "taskId": "job_a", "pid": 42, "startedAt": 100}),
        json!({"task_kind": "local_bash", "cmd": ["echo", "hi"], "status": "killed",
               "jobId": "job_b", "cancellable": "yes"}),
    ]);
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0]["jobId"], json!("job_a"));
    assert_eq!(jobs[0]["taskKind"], json!("agent"));
    assert_eq!(jobs[0]["command"], json!("跑一遍迁移"));
    assert_eq!(jobs[0]["status"], json!("running"));
    assert_eq!(jobs[0]["pid"], json!(42));
    assert_eq!(jobs[1]["taskKind"], json!("bash"));
    assert_eq!(jobs[1]["command"], json!("echo hi"));
    assert_eq!(jobs[1]["status"], json!("killed"));
    assert_eq!(jobs[1]["cancellable"], json!(true));
    // 派生 jobId：taskKind 与命令都有但无显式 id。
    let derived = parse_control_items(&[json!({"type": "background_bash", "command": "sleep 1"})]);
    assert_eq!(
        derived[0]["jobId"],
        json!("background-task:no-pid:no-start:sleep 1")
    );
}

#[test]
fn control_items_skip_unknown_kind_and_missing_command() {
    assert!(parse_control_items(&[json!({"status": "running"})]).is_empty());
    assert!(parse_control_items(&[json!({"taskKind": "bash"})]).is_empty());
    assert!(parse_control_items(&[json!("not-an-object")]).is_empty());
    // 同一 jobId 后者覆盖，且保持首次出现的位置。
    let jobs = parse_control_items(&[
        json!({"taskKind": "bash", "command": "a", "jobId": "same", "status": "running"}),
        json!({"taskKind": "bash", "command": "b", "jobId": "same", "status": "completed"}),
    ]);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["command"], json!("b"));
    assert_eq!(jobs[0]["status"], json!("completed"));
}

#[test]
fn iso_instant_parsing_matches_date_parse_for_the_supported_subset() {
    assert_eq!(parse_iso_instant_ms("1970-01-01T00:00:00.000Z"), Some(0.0));
    assert_eq!(parse_iso_instant_ms("1970-01-01T00:00:01Z"), Some(1000.0));
    // 2024-03-01T12:30:00+01:00 = 2024-03-01T11:30:00Z = 1709292600000（`Date.parse` 同值）。
    assert_eq!(
        parse_iso_instant_ms("2024-03-01T12:30:00+01:00"),
        Some(1_709_292_600_000.0)
    );
    assert_eq!(parse_iso_instant_ms("not-a-date"), None);
}
