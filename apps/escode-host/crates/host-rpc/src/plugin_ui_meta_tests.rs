//! `plugin_ui_meta` 单测（TS `pickHtmlResource` / `normalizeMcpAppResourceMeta` 的对照语料）。
use super::*;
use serde_json::json;

#[test]
fn picks_the_same_uri_html_item_and_ignores_charset() {
    let result = json!({"contents":[
        {"uri":"ui://other/index.html","mimeType":"text/html","text":"<other/>"},
        {"uri":"ui://app/index.html","mimeType":"text/html; charset=utf-8","text":"<app/>","_meta":{"ui":{"prefersBorder":true}}}
    ]});
    let picked = pick_html(&result, "ui://app/index.html").expect("picked");
    assert_eq!(picked.html, "<app/>");
    assert_eq!(picked.meta.unwrap()["prefersBorder"], json!(true));
}

#[test]
fn normalizes_modern_and_legacy_resource_meta() {
    let meta = json!({
        "ui": {
            "csp": {"connectDomains":["https://api.example.com","http://127.0.0.1:4100"]},
            "permissions": {"camera": {}, "unknown": {}},
            "showInline": true
        },
        "openai/widgetPrefersBorder": true,
        "openai/widgetHeightHint": 320.4,
        "escode/csp": {"unsafeEval": true, "wasmUnsafeEval": false}
    });
    let normalized = normalize_resource_meta(&meta).expect("meta");
    assert_eq!(
        normalized["csp"]["connectDomains"][0],
        "https://api.example.com"
    );
    assert_eq!(
        normalized["csp"]["connectDomains"][1],
        "http://127.0.0.1:4100"
    );
    assert_eq!(normalized["prefersBorder"], json!(true));
    assert_eq!(normalized["heightHint"], json!(320));
    assert_eq!(normalized["showInline"], json!(true));
    assert_eq!(normalized["permissions"], json!(["camera"]));
    assert_eq!(normalized["cspRelaxations"], json!({"unsafeEval": true}));
}

#[test]
fn reads_legacy_snake_case_csp_aliases() {
    let meta = json!({"openai/widgetCSP": {"resource_domains": ["https://cdn.example.com"]}});
    let normalized = normalize_resource_meta(&meta).expect("meta");
    assert_eq!(
        normalized["csp"]["resourceDomains"],
        json!(["https://cdn.example.com"])
    );
}

#[test]
fn empty_or_unrelated_meta_is_none() {
    assert!(normalize_resource_meta(&json!({})).is_none());
    assert!(normalize_resource_meta(&json!(null)).is_none());
    assert!(normalize_resource_meta(&json!({"ui": {}})).is_none());
}

#[test]
fn rejects_non_html_mime_and_blob_only_payloads() {
    let wrong =
        json!({"contents":[{"uri":"ui://app/x","mimeType":"application/json","text":"{}"}]});
    assert!(pick_html(&wrong, "ui://app/x").is_err());
    let blob = json!({"contents":[{"uri":"ui://app/x","mimeType":"text/html","blob":"AAAA"}]});
    assert!(pick_html(&blob, "ui://app/x").is_err());
    assert!(pick_html(&json!({"contents":[]}), "ui://app/x").is_err());
}

#[test]
fn origin_normalization_is_loopback_only_for_plain_http() {
    assert_eq!(
        normalize_origin("https://api.example.com").as_deref(),
        Some("https://api.example.com")
    );
    assert_eq!(
        normalize_origin("http://127.0.0.1:4100").as_deref(),
        Some("http://127.0.0.1:4100")
    );
    assert!(normalize_origin("http://example.com").is_none());
    assert!(normalize_origin("https://example.com/path").is_none());
    assert!(normalize_origin("https://user:pass@example.com").is_none());
    assert!(normalize_origin("https://example.com?q=1").is_none());
}
