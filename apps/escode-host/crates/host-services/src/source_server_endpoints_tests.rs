use super::{authenticated_url, resolve_server_remote_endpoints};

fn endpoints(url: &str) -> (String, String, String, String) {
    let resolved = resolve_server_remote_endpoints(url).expect("endpoints");
    (
        resolved.info_url,
        resolved.ws_url,
        resolved.host_capability_url,
        resolved.host_ws_url,
    )
}

#[test]
fn https_base_path_derives_wss_endpoints() {
    // 与 TS `serverRemoteConnection.test.ts` 的 https 用例逐字节一致。
    assert_eq!(
        endpoints("https://studio.example.com/escode/"),
        (
            "https://studio.example.com/escode/api/server-info".to_owned(),
            "wss://studio.example.com/escode/ws".to_owned(),
            "https://studio.example.com/escode/api/rpc-host-capability".to_owned(),
            "wss://studio.example.com/escode/ws/host".to_owned(),
        )
    );
}

#[test]
fn websocket_input_keeps_ws_and_derives_http() {
    assert_eq!(
        endpoints("ws://127.0.0.1:3030/ws"),
        (
            "http://127.0.0.1:3030/api/server-info".to_owned(),
            "ws://127.0.0.1:3030/ws".to_owned(),
            "http://127.0.0.1:3030/api/rpc-host-capability".to_owned(),
            "ws://127.0.0.1:3030/ws/host".to_owned(),
        )
    );
}

#[test]
fn host_websocket_input_strips_host_path() {
    assert_eq!(
        endpoints("http://127.0.0.1:3030/ws/host/"),
        (
            "http://127.0.0.1:3030/api/server-info".to_owned(),
            "ws://127.0.0.1:3030/ws".to_owned(),
            "http://127.0.0.1:3030/api/rpc-host-capability".to_owned(),
            "ws://127.0.0.1:3030/ws/host".to_owned(),
        )
    );
}

#[test]
fn secure_websocket_input_derives_https() {
    assert_eq!(
        endpoints("wss://studio.example.com/base/ws"),
        (
            "https://studio.example.com/base/api/server-info".to_owned(),
            "wss://studio.example.com/base/ws".to_owned(),
            "https://studio.example.com/base/api/rpc-host-capability".to_owned(),
            "wss://studio.example.com/base/ws/host".to_owned(),
        )
    );
}

#[test]
fn query_and_fragment_are_dropped() {
    assert_eq!(
        endpoints("https://studio.example.com/base/?token=leak#frag"),
        (
            "https://studio.example.com/base/api/server-info".to_owned(),
            "wss://studio.example.com/base/ws".to_owned(),
            "https://studio.example.com/base/api/rpc-host-capability".to_owned(),
            "wss://studio.example.com/base/ws/host".to_owned(),
        )
    );
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    assert_eq!(
        endpoints("  http://127.0.0.1:3030  ").0,
        "http://127.0.0.1:3030/api/server-info"
    );
}

#[test]
fn blank_url_is_rejected() {
    for value in ["", "   ", "\n"] {
        let error = resolve_server_remote_endpoints(value).expect_err("blank url");
        assert!(
            error.to_string().contains("server url is required"),
            "{error}"
        );
    }
}

#[test]
fn unsupported_protocol_is_rejected() {
    let error = resolve_server_remote_endpoints("ftp://example.com").expect_err("ftp");
    assert!(
        error
            .to_string()
            .contains("unsupported server url protocol"),
        "{error}"
    );
}

#[test]
fn relative_url_is_rejected() {
    assert!(resolve_server_remote_endpoints("example.com/escode").is_err());
}

#[test]
fn authenticated_url_appends_token_query() {
    assert_eq!(
        authenticated_url("http://127.0.0.1:3030/api/server-info", Some(" s3cret ")).expect("url"),
        "http://127.0.0.1:3030/api/server-info?token=s3cret"
    );
}

#[test]
fn authenticated_url_replaces_existing_token() {
    assert_eq!(
        authenticated_url("http://127.0.0.1:3030/ws?token=old&keep=1", Some("new")).expect("url"),
        "http://127.0.0.1:3030/ws?keep=1&token=new"
    );
}

#[test]
fn authenticated_url_without_token_is_unchanged() {
    let url = "wss://studio.example.com/base/ws/host";
    assert_eq!(authenticated_url(url, None).expect("url"), url);
    assert_eq!(authenticated_url(url, Some("  ")).expect("url"), url);
}
