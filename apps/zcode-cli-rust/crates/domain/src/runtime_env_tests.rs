use super::*;

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
}

/// 把差量顺序应用到父环境，得到子进程最终环境（大小写敏感，对应非 Windows）。
fn apply(parent: &[(String, String)], changes: &[(String, Option<String>)]) -> Vec<(String, String)> {
    let mut out = parent.to_vec();
    for (key, value) in changes {
        out.retain(|(k, _)| k != key);
        if let Some(value) = value {
            out.push((key.clone(), value.clone()));
        }
    }
    out.sort();
    out
}

#[test]
fn sanitizes_runtime_keys_for_internal_children() {
    let parent = env(&[
        ("PATH", "/bin"),
        ("NODE_ENV", "production"),
        ("http_proxy", "http://p"),
        ("npm_config_proxy", "http://n"),
        ("OTEL_SERVICE_NAME", "zcode"),
        (CUA_SOCKET_KEY, "/s"),
        (CUA_AUTHORITY_KEY, "a"),
    ]);
    assert_eq!(apply(&parent, &child_env(&parent, false, false)), env(&[("PATH", "/bin")]));
}

#[test]
fn tool_children_restore_passthrough_and_drop_credentials() {
    let parent = env(&[
        ("PATH", "/bin"),
        ("NODE_ENV", "production"),
        ("HTTPS_PROXY", "http://user-proxy"),
        ("SSL_CERT_DIR", "/certs"),
        ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=x"),
        (CUA_SOCKET_KEY, "/s"),
        (CUA_TOKEN_KEY, "legacy"),
        (TOOL_ENV_PASSTHROUGH_KEY, r#"{"NO_PROXY":"a.test","HTTPS_PROXY":"http://old","PATH":"/evil","NODE_ENV":"x","bad-key":"y"}"#),
    ]);
    assert_eq!(
        apply(&parent, &child_env(&parent, true, false)),
        env(&[
            ("HTTPS_PROXY", "http://user-proxy"),
            ("NO_PROXY", "a.test"),
            ("PATH", "/bin"),
            ("SSL_CERT_DIR", "/certs"),
        ])
    );
}

#[test]
fn zcode_network_keys_override_all_variants() {
    let parent = env(&[
        ("http_proxy", "http://user"),
        ("ZCODE_HTTP_PROXY", " 127.0.0.1:7890 "),
        ("ZCODE_NO_PROXY", "localhost"),
        ("ZCODE_AGENT_CA_CERT", "/ca.pem"),
    ]);
    let child = apply(&parent, &child_env(&parent, true, false));
    let get = |key: &str| child.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    for key in PROXY_KEYS {
        assert_eq!(get(key), Some("http://127.0.0.1:7890"), "{key}");
    }
    for key in NO_PROXY_KEYS {
        assert_eq!(get(key), Some("localhost"));
    }
    for key in CA_KEYS {
        assert_eq!(get(key), Some("/ca.pem"));
    }
    assert_eq!(get("SSL_CERT_DIR"), None);
    assert_eq!(get("ZCODE_HTTP_PROXY"), Some(" 127.0.0.1:7890 "));
}

#[test]
fn windows_removes_case_variants_before_setting() {
    let parent = env(&[("Http_Proxy", "http://x"), ("ZCODE_HTTP_PROXY", "socks5://p:1")]);
    let changes = child_env(&parent, true, true);
    assert!(changes.contains(&("Http_Proxy".into(), None)));
    assert!(changes.iter().any(|(k, v)| k == "all_proxy" && v.as_deref() == Some("socks5://p:1")));
}

#[test]
fn cua_credentials_require_socket_and_authority() {
    assert_eq!(cua_credentials(&env(&[(CUA_SOCKET_KEY, "/s")])), None);
    assert_eq!(cua_credentials(&env(&[(CUA_SOCKET_KEY, "/s"), (CUA_AUTHORITY_KEY, " ")])), None);
    assert_eq!(
        cua_credentials(&env(&[(CUA_SOCKET_KEY, " /s "), (CUA_AUTHORITY_KEY, "a"), (CUA_REFRESH_MARKER_KEY, "m")])),
        Some(CuaCredentials {
            socket: "/s".into(),
            authority: "a".into(),
            refresh_marker: Some("m".into())
        })
    );
}

#[test]
fn package_manager_pattern_is_case_insensitive() {
    assert!(is_sanitized("NPM_CONFIG_HTTPS_PROXY"));
    assert!(is_sanitized("pnpm_cafile"));
    assert!(!is_sanitized("npm_config_registry"));
    assert!(is_passthrough_capturable("yarn_proxy"));
    assert!(!is_passthrough_capturable("ZCODE_TELEMETRY_DEVICE_MID"));
}
