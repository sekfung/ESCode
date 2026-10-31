use std::collections::HashSet;

use serde_json::{Value, json};

use super::tool_search_announcement::*;
use crate::tool_search_catalog::ResolvedToolSearchMode;

fn available(names: &[&str]) -> HashSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn announcement(names: &[&str], deferred: &[&str], messages: &[Value]) -> Option<Value> {
    build(
        messages,
        &deferred.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>(),
        &available(names),
        ResolvedToolSearchMode::Native,
    )
}

#[test]
fn announces_native_additions_with_exact_copy() {
    let message = announcement(
        &["ToolSearch", "WebFetch"],
        &["WebFetch"],
        &[],
    )
    .unwrap();
    assert_eq!(
        message["content"],
        json!("<system-reminder>\nThe following deferred tools are now available via ToolSearch. Their schemas are NOT loaded — calling them directly will fail with InputValidationError. Use ToolSearch with query \"select:<name>[,<name>...]\" to load tool schemas before calling them:\nWebFetch\n</system-reminder>")
    );
    assert_eq!(
        message[ANNOUNCEMENT_KEY],
        json!({"addedNames": ["WebFetch"], "removedNames": [], "readdedNames": []})
    );
    assert_eq!(message["_escode_source"], ANNOUNCEMENT_SOURCE);
    assert!(announcement(&["ToolSearch", "WebFetch"], &["WebFetch"], &[message]).is_none());
}

#[test]
fn uses_local_copy_mentioning_deferred_tool_call() {
    let message = build(
        &[],
        &["WebFetch".to_owned()],
        &available(&["ToolSearch", "WebFetch"]),
        ResolvedToolSearchMode::Local,
    )
    .unwrap();
    let content = message["content"].as_str().unwrap();
    assert!(content.contains("Call resident tools directly:\nWebFetch"));
    assert!(content.contains("call them through DeferredToolCall"));
}

#[test]
fn ignores_announcements_in_off_mode() {
    assert!(
        build(
            &[],
            &["WebFetch".to_owned()],
            &available(&["ToolSearch", "WebFetch"]),
            ResolvedToolSearchMode::Off,
        )
        .is_none()
    );
}

#[test]
fn rebuilds_listing_from_retained_deltas_and_reports_readded_only_suffix() {
    let names = ["ToolSearch", "WebFetch", "TodoWrite"];
    let first = announcement(&names, &["WebFetch"], &[]).unwrap();
    let second = announcement(&names, &["WebFetch", "TodoWrite"], std::slice::from_ref(&first)).unwrap();
    assert_eq!(
        second[ANNOUNCEMENT_KEY],
        json!({"addedNames": ["TodoWrite"], "removedNames": [], "readdedNames": []})
    );
    let rebuilt = announcement(&names, &["WebFetch", "TodoWrite"], std::slice::from_ref(&second)).unwrap();
    assert_eq!(
        rebuilt[ANNOUNCEMENT_KEY],
        json!({"addedNames": ["WebFetch"], "removedNames": [], "readdedNames": []})
    );
    assert!(rebuilt["content"].as_str().unwrap().contains("\nWebFetch\n"));

    let removed = announcement(
        &["ToolSearch", "TodoWrite"],
        &["TodoWrite"],
        &[first.clone(), second.clone()],
    )
    .unwrap();
    assert_eq!(
        removed[ANNOUNCEMENT_KEY],
        json!({"addedNames": [], "removedNames": ["WebFetch"], "readdedNames": []})
    );
    let readded = announcement(
        &["ToolSearch", "WebFetch", "TodoWrite"],
        &["WebFetch", "TodoWrite"],
        &[first, second, removed],
    )
    .unwrap();
    assert_eq!(
        readded[ANNOUNCEMENT_KEY],
        json!({"addedNames": ["WebFetch"], "removedNames": [], "readdedNames": ["WebFetch"]})
    );
    assert_eq!(
        readded["content"],
        json!("<system-reminder>\n1 deferred tool is available again (names announced earlier in this conversation): WebFetch. Load via ToolSearch as before.\n</system-reminder>")
    );
}

#[test]
fn renders_removals_at_the_thirty_name_threshold() {
    for count in [30usize, 31] {
        let names: Vec<String> = (0..count)
            .map(|index| format!("mcp__demo__tool{index:02}"))
            .collect();
        let deferred: Vec<String> = names.clone();
        let first = build(
            &[],
            &deferred,
            &available(&names.iter().map(String::as_str).collect::<Vec<_>>()),
            ResolvedToolSearchMode::Native,
        )
        .unwrap();
        let tool_search: HashSet<String> = available(&["ToolSearch"]);
        let removed = build(
            &[first],
            &[],
            &tool_search,
            ResolvedToolSearchMode::Native,
        )
        .unwrap();
        let expected = if count == 30 {
            format!(
                "The following deferred tools are no longer available. Do not search for them — ToolSearch will return no match:\n{}",
                names.join("\n")
            )
        } else {
            "31 deferred tools are no longer available: mcp__demo__* (31). Do not search for them — ToolSearch will return no match.".to_owned()
        };
        assert_eq!(
            removed["content"],
            json!(format!(
                "<system-reminder>\n{expected}\n\n{AMBIENT_CONTEXT_NOTICE}\n</system-reminder>"
            ))
        );
        assert_eq!(
            removed[ANNOUNCEMENT_KEY]["removedNames"],
            json!(names)
        );
    }
}

#[test]
fn separates_added_readded_and_removed_paragraphs() {
    let names = ["mcp__demo__read", "mcp__demo__write", "mcp__other__read", "TodoWrite"];
    let prior = json!({"addedNames": ["mcp__demo__read", "mcp__demo__write", "mcp__other__read", "TodoWrite", "Gone"], "removedNames": [], "readdedNames": []});
    let removal = json!({"addedNames": [], "removedNames": names, "readdedNames": []});
    let entries = [
        json!({"_escode_source": "legacy_synthetic", ANNOUNCEMENT_KEY: prior}),
        json!({"_escode_source": "legacy_synthetic", ANNOUNCEMENT_KEY: removal}),
    ];
    let deferred = ["WebFetch", "mcp__demo__read", "mcp__demo__write", "mcp__other__read", "TodoWrite"];
    let message = announcement(
        &["ToolSearch", "WebFetch", "mcp__demo__read", "mcp__demo__write", "mcp__other__read", "TodoWrite"],
        &deferred,
        &entries,
    )
    .unwrap();
    assert_eq!(
        message["content"],
        json!(format!("<system-reminder>\nThe following deferred tools are now available via ToolSearch. Their schemas are NOT loaded — calling them directly will fail with InputValidationError. Use ToolSearch with query \"select:<name>[,<name>...]\" to load tool schemas before calling them:\nWebFetch\n\n4 deferred tools are available again (names announced earlier in this conversation): mcp__demo__* (2), mcp__other__*, TodoWrite. Load via ToolSearch as before.\n\nThe following deferred tools are no longer available. Do not search for them — ToolSearch will return no match:\nGone\n\n{AMBIENT_CONTEXT_NOTICE}\n</system-reminder>"))
    );
}

#[test]
fn collects_discovered_names_from_tool_references_and_boundaries() {
    let messages = [
        json!({"role": "user", "content": "hi"}),
        json!({"role": "tool", "content": [{"type": "text", "text": "ok"}, {"type": "tool_reference", "toolName": "mcp__demo__read"}]}),
        json!({"role": "tool", "_escode_tool_failed": true, "content": [{"type": "tool_reference", "toolName": "Failed"}]}),
        json!({"role": "user", "_escode_source": "todo_reminder", "content": [{"type": "tool_reference", "toolName": "Reminder"}]}),
        json!({"role": "user", "metadata": {"compactBoundary": {"preCompactDiscoveredTools": ["CompactTool"]}}}),
    ];
    assert_eq!(
        discovered(&messages),
        vec!["mcp__demo__read".to_owned(), "CompactTool".to_owned()]
    );
}

#[test]
fn collects_discovered_names_from_local_tool_search_results() {
    // local 结果只有文本 schema 块；命中名由 `_escode_tool_search_discovered` 持久化，
    // 冷恢复（无压缩边界）也必须能重建已发现集合。
    let messages = [
        json!({"role": "tool", DISCOVERED_KEY: ["mcp__demo__read", "mcp__demo__write"], "content": [{"type": "text", "text": "<functions>…</functions>"}]}),
        json!({"role": "tool", "_escode_tool_failed": true, DISCOVERED_KEY: ["Failed"], "content": "error"}),
        json!({"role": "tool", DISCOVERED_KEY: ["mcp__demo__read"], "content": "No matching deferred tools found"}),
    ];
    assert_eq!(
        discovered(&messages),
        vec!["mcp__demo__read".to_owned(), "mcp__demo__write".to_owned()]
    );
}

#[test]
fn groups_mcp_names_case_insensitively() {
    assert_eq!(
        summarize(&[
            "TodoWrite".to_owned(),
            "mcp__demo__write".to_owned(),
            "mcp__demo__read".to_owned(),
            "mcp__other__read".to_owned(),
        ]),
        "mcp__demo__* (2), mcp__other__*, TodoWrite"
    );
}

#[test]
fn bubbles_announcement_before_the_current_user_input() {
    let mut messages = vec![
        json!({"role": "user", "content": "hi"}),
        json!({"role": "user", "_escode_source": "todo_reminder", "content": "todo"}),
        json!({"role": "user", "_escode_source": ANNOUNCEMENT_SOURCE, "content": "notice"}),
    ];
    order_before_input(&mut messages);
    assert_eq!(messages[0]["content"], "notice");
    assert_eq!(messages[1]["content"], "hi");
    assert_eq!(messages[2]["content"], "todo");

    // 工具结果之后的公告属于该步骤，不越过工具边界。
    let mut after_tool = vec![
        json!({"role": "user", "content": "hi"}),
        json!({"role": "tool", "content": "result"}),
        json!({"role": "user", "_escode_source": ANNOUNCEMENT_SOURCE, "content": "notice"}),
    ];
    order_before_input(&mut after_tool);
    assert_eq!(after_tool[2]["content"], "notice");
}
