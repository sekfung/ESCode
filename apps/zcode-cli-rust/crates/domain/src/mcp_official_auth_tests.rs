use super::*;
use serde_json::{Value, json};

fn corpus() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/mcp_official_auth_corpus.json")).unwrap()
}

#[test]
fn origin_trust_matches_ts() {
    for case in corpus()["trust"].as_array().unwrap() {
        let (trusted, detail) = origin_trusted(
            case["origin"].as_str().unwrap(),
            case["devTrustedOriginsRaw"].as_str(),
            case["zcodeApiOrigin"].as_str(),
        );
        assert_eq!(json!({"trusted": trusted, "detail": detail}), case["result"], "{case}");
    }
}

#[test]
fn reserved_headers_match_ts() {
    for case in corpus()["reserved"].as_array().unwrap() {
        let names = case["headers"].as_object().unwrap().keys().map(String::as_str);
        assert_eq!(json!(reserved_headers(names)), case["result"], "{case}");
    }
}

#[test]
fn zcode_origin_matches_ts() {
    for case in corpus()["origins"].as_array().unwrap() {
        let result = zcode_api_origin(case["baseUrl"].as_str(), case["endpointOrigin"].as_str());
        let expected = &case["result"];
        match result {
            Ok(origin) => assert_eq!(json!({"ok": origin}), *expected, "{case}"),
            Err(_) => assert!(expected.get("error").is_some(), "{case}"),
        }
    }
}

#[test]
fn auth_parsing_matches_ts() {
    for case in corpus()["auth"].as_array().unwrap() {
        let value = (case["value"] != "<undefined>").then_some(&case["value"]);
        let result = parse_auth(value, "plugin:server");
        let expected = &case["result"];
        match result {
            Ok(official) => assert_eq!(official, !expected["ok"].is_null(), "{case}"),
            Err(message) => assert_eq!(json!(message), expected["error"], "{case}"),
        }
    }
}

#[test]
fn merge_overrides_identity_and_drops_reserved_and_correlation_headers() {
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    let incoming = [
        s("Authorization", "Bearer oauth"),
        s("X-Coding-Plan-Api-Key", "k"),
        s("Mcp-Session-Id", "sid"),
        s("mcp-protocol-version", "v"),
        s("X-Request-Id", "r"),
        s("x-trace-id", "t"),
        s("Content-Type", "application/json"),
        s("x-bigmodel-authorization", "stale"),
    ];
    let auth = [s("Authorization", "Bearer jwt"), s("X-Bigmodel-Authorization", "plan")];
    assert_eq!(
        merge_headers(&incoming, &auth),
        vec![
            s("Mcp-Session-Id", "sid"),
            s("mcp-protocol-version", "v"),
            s("Content-Type", "application/json"),
            s("Authorization", "Bearer jwt"),
            s("X-Bigmodel-Authorization", "plan"),
        ]
    );
}

#[test]
fn response_classification_follows_ts() {
    let json_ct = "application/json; charset=utf-8";
    assert_eq!(classify_response(429, "", None), Some("rate_limited"));
    assert_eq!(classify_response(502, json_ct, None), Some("server_internal_error"));
    assert_eq!(classify_response(200, "text/event-stream", None), None);
    assert_eq!(classify_response(404, "text/plain", None), Some("connection_failed"));
    assert_eq!(classify_response(200, json_ct, Some(r#"{"code":3001}"#)), Some("server_not_found"));
    assert_eq!(classify_response(400, json_ct, Some(r#"{"code":1000}"#)), Some("server_unavailable"));
    let rpc = |code: i64| format!(r#"{{"jsonrpc":"2.0","id":1,"error":{{"code":{code}}}}}"#);
    assert_eq!(classify_response(200, json_ct, Some(&rpc(1006))), Some("not_authenticated"));
    assert_eq!(classify_response(200, json_ct, Some(&rpc(3101))), Some("coding_plan_required"));
    assert_eq!(classify_response(200, json_ct, Some(&rpc(-32601))), Some("protocol_error"));
    assert_eq!(classify_response(200, json_ct, Some(r#"{"jsonrpc":"2.0","result":{}}"#)), None);
    assert_eq!(classify_response(400, json_ct, Some("not json")), Some("connection_failed"));
    assert_eq!(classify_response(400, json_ct, Some("[1]")), Some("connection_failed"));
    assert_eq!(auth_failure(401), Some("official_auth_rejected"));
    assert_eq!(auth_failure(403), Some("official_auth_forbidden"));
    assert_eq!(auth_failure(307), Some("official_auth_redirect_blocked"));
    assert_eq!(auth_failure(200), None);
}
