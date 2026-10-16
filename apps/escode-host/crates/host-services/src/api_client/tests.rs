use std::sync::Mutex;

use async_trait::async_trait;

use super::*;

/// 脚本化应答：`(状态码, 正文, 头)` 或网络错误。
type Scripted = Result<(u16, Vec<u8>, BTreeMap<String, String>), String>;

/// 记录请求并按脚本回放的传输桩。
struct Stub {
    requests: Mutex<Vec<ApiRequest>>,
    reply: Mutex<Scripted>,
    delay: Option<Duration>,
}

impl Stub {
    fn ok(body: &str) -> Arc<Self> {
        Self::reply(200, body, &[])
    }

    fn reply(status: u16, body: &str, headers: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::default(),
            reply: Mutex::new(Ok((
                status,
                body.as_bytes().to_vec(),
                headers
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
            ))),
            delay: None,
        })
    }

    fn seen(&self) -> Vec<(String, BTreeMap<String, String>)> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| (request.url.to_string(), request.headers.clone()))
            .collect()
    }
}

#[async_trait]
impl Transport for Stub {
    async fn get(&self, request: ApiRequest) -> Result<ApiResponse, String> {
        self.requests.lock().unwrap().push(request);
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        match &*self.reply.lock().unwrap() {
            Ok((status, body, headers)) => Ok(ApiResponse {
                url: String::new(),
                status: *status,
                headers: headers.clone(),
                body: body.clone(),
            }),
            Err(message) => Err(message.clone()),
        }
    }
}

fn client(base: &str, transport: Arc<Stub>) -> ApiClient {
    let identity = ClientIdentity::from_env(
        |name| match name {
            "ESCODE_APP_VERSION" => Some("3.15.1"),
            "ESCODE_ENV" => Some("production"),
            "ESCODE_CLIENT_LANGUAGE" => Some("zh-CN"),
            "ESCODE_CLIENT_TIMEZONE" => Some("Asia/Shanghai"),
            "ESCODE_OS_VERSION" => Some("#1 SMP"),
            _ => None,
        },
        None,
    );
    ApiClient::new(
        EndpointSource {
            escode_env: Some("production".into()),
            base_origin: Some(base.into()),
            settings_home: None,
        },
        identity,
        transport,
    )
}

#[test]
fn rewrite_matches_the_ts_vectors() {
    // 向量由真实 TS `rewriteESCodeEndpointUrl` 运行得到（`node --import tsx` 直接调用，7 组）。
    let target = "https://api.example.com";
    assert_eq!(
        rewrite_endpoint_url("https://zcode.z.ai/api/v1/client/scenes?a=1#h", target),
        "https://api.example.com/api/v1/client/scenes?a=1#h"
    );
    assert_eq!(
        rewrite_endpoint_url("https://other.example.com/x", target),
        "https://other.example.com/x",
        "非默认 Origin 不改写"
    );
    assert_eq!(
        rewrite_endpoint_url("https://zcode.z.ai/x", "https://zcode.z.ai"),
        "https://zcode.z.ai/x",
        "目标就是默认 Origin 时原样"
    );
    assert_eq!(rewrite_endpoint_url("not a url", target), "not a url");
    assert_eq!(
        rewrite_endpoint_url("https://zcode.z.ai", target),
        "https://api.example.com/",
        "无路径的 Origin 补 `/`"
    );
    assert_eq!(
        rewrite_endpoint_url(
            "https://ZCODE.z.ai:443/a%20b?x=%2F",
            "https://api.example.com:8443"
        ),
        "https://api.example.com:8443/a%20b?x=%2F",
        "大小写/默认端口/百分号编码"
    );
    assert_eq!(
        rewrite_endpoint_url("https://zcode.z.ai/p", "http://localhost:8080/ignored"),
        "http://localhost:8080/p"
    );
}

#[tokio::test]
async fn requests_to_the_endpoint_get_source_headers_and_a_request_id() {
    let stub = Stub::ok(r#"{"code":0}"#);
    let api = client("https://api.example.com", Arc::clone(&stub));
    api.get_json(
        "https://zcode.z.ai/api/v1/client/scenes",
        &GetOptions::default(),
    )
    .await
    .unwrap();
    let (url, headers) = stub.seen().remove(0);
    assert_eq!(url, "https://api.example.com/api/v1/client/scenes");
    assert_eq!(headers["user-agent"], "ESCode/3.15.1");
    assert_eq!(headers["http-referer"], "https://api.example.com");
    assert_eq!(headers["x-title"], "ESCode@electron");
    assert_eq!(headers["x-zcode-app-version"], "3.15.1");
    assert_eq!(headers["x-release-channel"], "production");
    assert_eq!(headers["x-client-language"], "zh-CN");
    assert_eq!(headers["x-client-timezone"], "Asia/Shanghai");
    assert_eq!(headers["x-os-version"], "#1 SMP");
    assert!(headers["x-platform"].contains('-'));
    assert!(matches!(
        headers["x-os-category"].as_str(),
        "linux" | "macos" | "windows"
    ));
    assert_eq!(headers["x-request-id"].len(), 36, "UUID");
    assert!(!headers.contains_key("authorization") && !headers.contains_key("cookie"));
}

#[tokio::test]
async fn foreign_origins_only_get_a_request_id() {
    let stub = Stub::ok("{}");
    let api = client("https://api.example.com", Arc::clone(&stub));
    api.get_json("https://cdn.example.com/x.json", &GetOptions::default())
        .await
        .unwrap();
    let (_, headers) = stub.seen().remove(0);
    assert_eq!(headers.keys().collect::<Vec<_>>(), ["x-request-id"]);
}

#[tokio::test]
async fn caller_headers_win_over_source_headers() {
    let stub = Stub::ok("{}");
    let api = client("https://api.example.com", Arc::clone(&stub));
    let options = GetOptions {
        headers: BTreeMap::from([
            ("user-agent".to_owned(), "custom/1".to_owned()),
            ("x-request-id".to_owned(), "fixed-id".to_owned()),
            (
                "http-referer".to_owned(),
                "https://elsewhere.example".to_owned(),
            ),
        ]),
        ..GetOptions::default()
    };
    api.get_json("https://api.example.com/x", &options)
        .await
        .unwrap();
    let (_, headers) = stub.seen().remove(0);
    assert_eq!(headers["user-agent"], "custom/1");
    assert_eq!(headers["x-request-id"], "fixed-id");
    assert_eq!(
        headers["http-referer"], "https://elsewhere.example",
        "调用方改过的 Referer 不再被换成 Endpoint"
    );
}

#[tokio::test]
async fn http_errors_carry_the_best_message_status_and_diagnostic_headers() {
    for (status, body, expected) in [
        (500, r#"{"error":"boom","message":"ignored"}"#, "boom"),
        (400, r#"{"message":"bad"}"#, "bad"),
        (403, r#"{"msg":"nope"}"#, "nope"),
        (404, r#"{"detail":"missing"}"#, "missing"),
        (502, "  upstream down \n", "upstream down"),
        (503, "", "HTTP 503"),
        (500, "null", "null"),
        (500, "5", "HTTP 500"),
        (500, r#"{"error":"   "}"#, "HTTP 500"),
        (500, r#"{"error":7,"message":"seven"}"#, "seven"),
    ] {
        let stub = Stub::reply(
            status,
            body,
            &[
                ("x-request-id", " req-1 "),
                ("x-trace-id", ""),
                ("x-other", "no"),
            ],
        );
        let api = client("https://api.example.com", stub);
        let error = api
            .get_json("https://api.example.com/x", &GetOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.message, expected, "{status} {body:?}");
        assert_eq!(error.status, Some(status));
        assert_eq!(
            error.response_headers,
            BTreeMap::from([("x-request-id".to_owned(), "req-1".to_owned())]),
            "只带非空的诊断头"
        );
    }
}

#[tokio::test]
async fn invalid_json_and_network_failures_become_api_errors() {
    let api = client("https://api.example.com", Stub::ok("<html>"));
    let error = api
        .get_json("https://api.example.com/x", &GetOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(200));
    let failing = Arc::new(Stub {
        requests: Mutex::default(),
        reply: Mutex::new(Err("connection refused".into())),
        delay: None,
    });
    let error = client("https://api.example.com", failing)
        .get_json("https://api.example.com/x", &GetOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        (error.message.as_str(), error.status),
        ("connection refused", None)
    );
}

#[tokio::test]
async fn a_timeout_aborts_with_the_ts_message() {
    let slow = Arc::new(Stub {
        requests: Mutex::default(),
        reply: Mutex::new(Ok((200, b"{}".to_vec(), BTreeMap::new()))),
        delay: Some(Duration::from_millis(200)),
    });
    let options = GetOptions {
        timeout: Some(Duration::from_millis(20)),
        ..GetOptions::default()
    };
    let error = client("https://api.example.com", slow)
        .get("https://api.example.com/x", &options)
        .await
        .err()
        .unwrap();
    assert_eq!(error.message, "Request timed out after 20ms");
}

#[test]
fn header_values_must_be_printable_ascii() {
    for (input, expected) in [
        (Some("  v1.2 "), Some("v1.2")),
        (Some("中文"), None),
        (Some("a\tb"), None),
        (Some("   "), None),
        (None, None),
    ] {
        assert_eq!(normalize_value(input).as_deref(), expected, "{input:?}");
    }
}
