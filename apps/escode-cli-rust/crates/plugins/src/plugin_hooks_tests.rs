//! plugin_hooks.rs 的解析/校验单测（文案与顺序对齐 TS `inspectPluginHooks`）。
use super::plugin_hooks::{Inspection, Source, parse_source};
use escode_cli_domain::json_order::Json;
use serde_json::Value;

fn source(text: &str, wrapper: bool) -> Source {
    Source {
        raw: Json::parse(text).unwrap(),
        source_path: "/plugins/demo/hooks/hooks.json".to_owned(),
        wrapper,
    }
}

fn parse(text: &str) -> (Inspection, Vec<Value>) {
    let mut inspection = Inspection {
        details: vec![],
        events: vec![],
    };
    let mut diagnostics = vec![];
    parse_source(
        "demo@inline",
        source(text, true),
        &mut inspection,
        &mut diagnostics,
    );
    (inspection, diagnostics)
}

fn codes(diagnostics: &[Value]) -> Vec<(&str, &str)> {
    diagnostics
        .iter()
        .map(|item| {
            (
                item["code"].as_str().unwrap(),
                item["severity"].as_str().unwrap(),
            )
        })
        .collect()
}

#[test]
fn expands_one_detail_per_hook_in_source_order() {
    let (inspection, diagnostics) = parse(
        r#"{"hooks":{
            "SessionStart":[{"matcher":"startup","hooks":[
              {"type":"command","command":"echo start","statusMessage":"Starting","timeout":12.5,"async":true,"shell":true}
            ]}],
            "PreToolUse":[
              {"hooks":[{"type":"process","command":"node","args":["hook.js"],"timeoutMs":5000}]},
              {"matcher":"","hooks":[{"type":"command","command":"echo second","shell":"/bin/sh"}]}
            ]
        }}"#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(inspection.events, vec!["SessionStart", "PreToolUse"]);
    assert_eq!(
        inspection.details,
        vec![
            serde_json::json!({
                "command": "echo start", "event": "SessionStart", "runnable": true,
                "sourcePath": "/plugins/demo/hooks/hooks.json", "type": "command",
                "matcher": "startup", "statusMessage": "Starting", "timeout": 12.5,
                "async": true, "shell": true,
            }),
            serde_json::json!({
                "command": "node", "event": "PreToolUse", "runnable": true,
                "sourcePath": "/plugins/demo/hooks/hooks.json", "type": "process",
                "timeoutMs": 5000, "args": ["hook.js"],
            }),
            serde_json::json!({
                "command": "echo second", "event": "PreToolUse", "runnable": true,
                "sourcePath": "/plugins/demo/hooks/hooks.json", "type": "command",
                "matcher": "", "shell": "/bin/sh",
            }),
        ]
    );
}

#[test]
fn unsupported_events_and_non_arrays_keep_the_event_name() {
    let (inspection, diagnostics) = parse(
        r#"{"hooks":{"NotAnEvent":[{"hooks":[{"type":"command","command":"x"}]}],"Stop":{"bad":true}}}"#,
    );
    assert_eq!(
        codes(&diagnostics),
        vec![
            ("plugin_hook_unsupported_event", "warning"),
            ("plugin_hook_invalid", "error"),
        ]
    );
    // listPluginHookEventNames 在数组校验之前就收录受支持事件名。
    assert_eq!(inspection.events, vec!["Stop"]);
    assert!(inspection.details.is_empty());
}

#[test]
fn invalid_matchers_are_reported_and_skip_details() {
    let (inspection, diagnostics) = parse(
        r#"{"hooks":{"Stop":[
            {"hooks":[]},
            {"hooks":[{"type":"command","command":""}]},
            {"hooks":[{"type":"command","command":"x","shell":false}]},
            {"hooks":[{"type":"command","command":"x","timeout":0}]},
            {"hooks":[{"type":"process","command":"x","timeoutMs":1.5}]},
            {"hooks":[{"type":"command","command":"x","enabled":"yes"}]},
            "nope"
        ]}}"#,
    );
    assert_eq!(inspection.events, vec!["Stop"]);
    assert!(inspection.details.is_empty());
    assert_eq!(
        codes(&diagnostics),
        vec![
            ("plugin_hook_invalid", "error"),
            ("plugin_hook_invalid", "error"),
            ("plugin_hook_invalid", "error"),
            ("plugin_hook_invalid", "error"),
            ("plugin_hook_invalid", "error"),
            ("plugin_hook_invalid", "error"),
            ("plugin_hook_invalid", "error"),
        ]
    );
    assert!(diagnostics.iter().all(|item| {
        item["message"]
            .as_str()
            .unwrap()
            .starts_with("Invalid plugin hook matcher for Stop: ")
    }));
}

#[test]
fn inline_manifest_hooks_are_not_unwrapped() {
    let mut inline = Inspection {
        details: vec![],
        events: vec![],
    };
    let mut inline_diagnostics = vec![];
    parse_source(
        "demo@inline",
        Source {
            raw: Json::parse(
                r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#,
            )
            .unwrap(),
            source_path: "/plugins/demo/.escode-plugin/plugin.json".to_owned(),
            wrapper: false,
        },
        &mut inline,
        &mut inline_diagnostics,
    );
    // 内联形态不会拆 `hooks` 包装：`hooks` 本身不是合法事件名（与 TS parsePluginHookEvents 一致）。
    assert_eq!(
        codes(&inline_diagnostics),
        vec![("plugin_hook_unsupported_event", "warning")]
    );
    assert!(inline.events.is_empty() && inline.details.is_empty());
}

#[test]
fn non_object_manifest_hooks_entry_is_an_error() {
    let mut inspection = Inspection {
        details: vec![],
        events: vec![],
    };
    let mut diagnostics = vec![];
    parse_source(
        "demo@inline",
        Source {
            raw: Json::Bool(true),
            source_path: "/plugins/demo/.escode-plugin/plugin.json".to_owned(),
            wrapper: false,
        },
        &mut inspection,
        &mut diagnostics,
    );
    assert_eq!(codes(&diagnostics), vec![("plugin_hook_invalid", "error")]);
    assert_eq!(
        diagnostics[0]["message"],
        "Plugin manifest hooks entry must be an object, a path, or an array"
    );
}
