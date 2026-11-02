use super::{display_ui, normalize_meta, plugin_id_for_server};
use serde_json::json;

#[test]
fn resource_uri_accepts_spec_legacy_and_openai_alias() {
    let spec = normalize_meta(&json!({"ui":{"resourceUri":" ui://server/index.html "}})).unwrap();
    assert_eq!(spec["resourceUri"], "ui://server/index.html");
    let legacy = normalize_meta(&json!({"ui/resourceUri":"ui://legacy"})).unwrap();
    assert_eq!(legacy["resourceUri"], "ui://legacy");
    let openai = normalize_meta(&json!({"openai/outputTemplate":"ui://alias"})).unwrap();
    assert_eq!(openai["resourceUri"], "ui://alias");
}

#[test]
fn invalid_resource_uri_drops_the_whole_descriptor() {
    assert!(normalize_meta(&json!({"ui":{"resourceUri":"https://x"}})).is_none());
    assert!(normalize_meta(&json!({"ui":{"resourceUri":""}})).is_none());
    assert!(normalize_meta(&json!({})).is_none());
}

#[test]
fn visibility_defaults_and_filters_unknown_values() {
    let default = normalize_meta(&json!({"ui":{"resourceUri":"ui://s"}})).unwrap();
    assert_eq!(default["visibility"], json!(["model", "app"]));
    let filtered =
        normalize_meta(&json!({"ui":{"resourceUri":"ui://s","visibility":["app","app","x"]}}))
            .unwrap();
    assert_eq!(filtered["visibility"], json!(["app"]));
}

#[test]
fn optional_fields_follow_openai_aliases_and_surface_pattern() {
    let descriptor = normalize_meta(&json!({
        "ui":{"resourceUri":"ui://s","prefersBorder":true,"surface":"board-1","showInline":true},
        "openai/ui":{"preferredModelDisplayMode":"pip"},
        "openai/widgetCSP":{"connectDomains":["https://api.example.com","http://127.0.0.1:7777"],
            "resource_domains":["ftp://example.com"]}
    }))
    .unwrap();
    assert_eq!(descriptor["preferredDisplayMode"], "pip");
    assert_eq!(descriptor["prefersBorder"], true);
    assert_eq!(descriptor["surface"], "board-1");
    assert_eq!(descriptor["showInline"], true);
    // 与 TS `readOriginList` 一致：数组存在即保留，`resource_domains` 全为非法 origin 时是空数组。
    assert_eq!(
        descriptor["csp"],
        json!({
            "connectDomains":["https://api.example.com","http://127.0.0.1:7777"],
            "resourceDomains":[]
        })
    );
    let bad_surface =
        normalize_meta(&json!({"ui":{"resourceUri":"ui://s","surface":"-bad"}})).unwrap();
    assert!(bad_surface.get("surface").is_none());
}

#[test]
fn display_ui_carries_result_payload_and_is_error() {
    let descriptor =
        normalize_meta(&json!({"ui":{"resourceUri":"ui://s","surface":"board"}})).unwrap();
    let ui = display_ui(
        &descriptor,
        "editor",
        Some(&json!({
            "structuredContent":{"a":1},
            "content":[{"type":"text","text":"hi"}],
            "isError":true
        })),
    )
    .unwrap();
    assert_eq!(ui["pluginId"], "editor");
    assert_eq!(ui["resourceUri"], "ui://s");
    assert_eq!(ui["surface"], "board");
    assert_eq!(ui["structuredContent"], "{\"a\":1}");
    assert_eq!(ui["content"], "[{\"text\":\"hi\",\"type\":\"text\"}]");
    assert_eq!(ui["isError"], true);
    assert!(ui.get("truncated").is_none());
}

#[test]
fn oversized_json_is_omitted_and_accounted() {
    let descriptor = normalize_meta(&json!({"ui":{"resourceUri":"ui://s"}})).unwrap();
    let big = "x".repeat(70 * 1024);
    let ui = display_ui(
        &descriptor,
        "s",
        Some(&json!({"structuredContent":{"blob":big}})),
    )
    .unwrap();
    assert!(ui.get("structuredContent").is_none());
    assert_eq!(ui["truncated"], true);
    assert!(ui["truncatedBytes"].as_u64().unwrap() > 70 * 1024);
}

#[test]
fn plugin_id_comes_from_plugin_server_names() {
    assert_eq!(plugin_id_for_server("plugin:editor:editor"), "editor");
    assert_eq!(plugin_id_for_server("plain-server"), "plain-server");
    assert_eq!(plugin_id_for_server("plugin::x"), "plugin::x");
}

#[test]
fn notification_only_for_successful_mcp_tool_rows() {
    let ui = json!({"pluginId":"editor","resourceUri":"ui://s"});
    let row = json!({"kind":"toolCall","status":"success","toolCallId":"tool",
        "input":{"file":"a.png"},"display":{"kind":"mcp_tool","serverName":"plugin:editor:editor","ui":ui}});
    let params = super::tool_result_notification("session-1", &row).unwrap();
    assert_eq!(params["sessionId"], "session-1");
    assert_eq!(params["toolCallId"], "tool");
    assert_eq!(params["serverName"], "plugin:editor:editor");
    assert_eq!(params["input"], json!({"file":"a.png"}));

    let mut failed = row.clone();
    failed["status"] = "error".into();
    assert!(super::tool_result_notification("session-1", &failed).is_none());

    let mut errored_ui = row.clone();
    errored_ui["display"]["ui"]["isError"] = true.into();
    assert!(super::tool_result_notification("session-1", &errored_ui).is_none());

    let mut no_ui = row.clone();
    no_ui["display"].as_object_mut().unwrap().remove("ui");
    assert!(super::tool_result_notification("session-1", &no_ui).is_none());

    let mut array_input = row;
    array_input["input"] = json!([1, 2]);
    let params = super::tool_result_notification("session-1", &array_input).unwrap();
    assert!(params.get("input").is_none());
}
