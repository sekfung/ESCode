//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-mcp-oauth-corpus.mjs）。
use super::*;

fn corpus() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/mcp_oauth_corpus.json")).unwrap()
}
fn config(value: &Value) -> CodeConfig {
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    CodeConfig {
        client_id: text("clientId"),
        client_secret: text("clientSecret"),
        client_name: text("clientName"),
        scope: text("scope"),
        redirect_path: text("redirectPath"),
    }
}

#[test]
fn key_prefixes_match_ts() {
    for case in corpus()["keyPrefixes"].as_array().unwrap() {
        let prefix = key_prefix(
            case["name"].as_str().unwrap(),
            case["url"].as_str().unwrap(),
            &config(&case["config"]),
        );
        assert_eq!(prefix, case["prefix"].as_str().unwrap());
        assert_eq!(sanitize_key_prefix(&prefix), case["sanitized"].as_str().unwrap());
    }
}

#[test]
fn credential_pairs_match_ts() {
    for case in corpus()["pairInputs"].as_array().unwrap() {
        let input = &case["input"];
        let pair = derive_pair(
            input["canonicalRaw"].as_str(),
            input["legacyClientRaw"].as_str(),
            input["legacyTokensRaw"].as_str(),
        );
        assert_eq!(pair.map(|p| p.to_json()).unwrap_or(Value::Null), case["pair"], "{input}");
    }
}

#[test]
fn scope_unions_match_ts() {
    for case in corpus()["scopeUnions"].as_array().unwrap() {
        let scopes: Vec<Option<&str>> =
            case["scopes"].as_array().unwrap().iter().map(Value::as_str).collect();
        assert_eq!(scope_union(&scopes).as_deref(), case["union"].as_str(), "{case}");
    }
}

#[test]
fn challenges_match_ts() {
    for case in corpus()["wwwAuthenticate"].as_array().unwrap() {
        let parsed = parse_challenge(case["header"].as_str().unwrap());
        let expected = &case["params"];
        assert_eq!(parsed.resource_metadata_url.as_deref(), expected["resourceMetadataUrl"].as_str(), "{case}");
        assert_eq!(parsed.scope.as_deref(), expected["scope"].as_str(), "{case}");
        assert_eq!(parsed.error.as_deref(), expected["error"].as_str(), "{case}");
        assert_eq!(parsed.error_description.as_deref(), expected["errorDescription"].as_str(), "{case}");
    }
}

#[test]
fn discovery_urls_match_ts() {
    for case in corpus()["discoveryUrls"].as_array().unwrap() {
        let urls: Vec<Value> = discovery_urls(case["url"].as_str().unwrap())
            .into_iter()
            .map(|(url, oidc)| json!({"url":url,"type":if oidc {"oidc"} else {"oauth"}}))
            .collect();
        assert_eq!(Value::Array(urls), case["urls"], "{case}");
    }
}

#[test]
fn resources_match_ts() {
    for case in corpus()["resources"].as_array().unwrap() {
        let requested = case["requested"].as_str().unwrap();
        assert_eq!(resource_url(requested).as_deref(), case["resource"].as_str(), "{case}");
        assert_eq!(
            resource_allowed(requested, case["configured"].as_str().unwrap()),
            case["allowed"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn client_auth_methods_match_ts() {
    for case in corpus()["authMethods"].as_array().unwrap() {
        let supported: Vec<&str> =
            case["supported"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(
            client_auth_method(&case["clientInformation"], &supported),
            case["method"].as_str().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn client_metadata_matches_ts() {
    for case in corpus()["clientMetadata"].as_array().unwrap() {
        let metadata = client_metadata(
            &config(&case["config"]),
            "srv",
            case["redirectUrl"].as_str().unwrap(),
            None,
        );
        assert_eq!(metadata, case["metadata"], "{case}");
    }
}

#[test]
fn code_config_follows_ts_resolution() {
    let plain = json!({"type":"http","url":"https://x"});
    assert_eq!(code_config(&plain, "http"), Some(CodeConfig::default()));
    assert_eq!(code_config(&plain, "stdio"), None);
    let header = json!({"headers":{"authorization":"Bearer x"}});
    assert_eq!(code_config(&header, "http"), None);
    let credentials = json!({"oauth":{"type":"client_credentials","clientId":"c"}});
    assert_eq!(code_config(&credentials, "sse"), None);
    let official = json!({"auth":{"type":"zcode_official","provider":"jwt_token"},"official":{}});
    assert_eq!(code_config(&official, "http"), None);
    // sse 不在官方鉴权形态内：官方字段只对 http/stdio 生效。
    assert_eq!(code_config(&official, "sse"), Some(CodeConfig::default()));
    let explicit = json!({"oauth":{"type":"authorization_code","clientId":"c","redirectPath":"cb"}});
    let resolved = code_config(&explicit, "http").unwrap();
    assert_eq!(resolved.client_id.as_deref(), Some("c"));
    assert_eq!(callback_path(resolved.redirect_path.as_deref(), "s"), "/cb");
    assert_eq!(callback_path(None, "my srv/名"), "/oauth/callback/mcp/my%20srv%2F%E5%90%8D");
}

#[test]
fn determine_scope_follows_sdk() {
    let offline = json!({"scopes_supported":["offline_access"]});
    let grants = ["authorization_code", "refresh_token"];
    assert_eq!(determine_scope(Some("a"), &Value::Null, &offline, None, &grants).as_deref(), Some("a offline_access"));
    assert_eq!(determine_scope(None, &json!({"scopes_supported":["r","w"]}), &Value::Null, Some("c"), &grants).as_deref(), Some("r w"));
    assert_eq!(determine_scope(None, &Value::Null, &offline, Some("c"), &["authorization_code"]).as_deref(), Some("c"));
    assert_eq!(determine_scope(None, &Value::Null, &offline, None, &grants), None);
}
