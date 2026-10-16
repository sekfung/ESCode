use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::service::{CACHE_TTL_MS, lock};
use super::*;
use crate::api_client::{ApiClient, ApiRequest, ApiResponse, ClientIdentity, Transport};
use crate::provider::EndpointSource;

/// 计数传输桩：`bodies` 逐次回放（用完后重复最后一个），可设延迟。
pub(super) struct Stub {
    pub(super) urls: Mutex<Vec<String>>,
    bodies: Mutex<Vec<(u16, String)>>,
    delay: Duration,
}

impl Stub {
    pub(super) fn new(bodies: Vec<(u16, String)>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            urls: Mutex::default(),
            bodies: Mutex::new(bodies),
            delay,
        })
    }

    pub(super) fn ok(body: Value) -> Arc<Self> {
        Self::new(vec![(200, body.to_string())], Duration::ZERO)
    }

    pub(super) fn count(&self) -> usize {
        lock(&self.urls).len()
    }
}

#[async_trait]
impl Transport for Stub {
    async fn get(&self, request: ApiRequest) -> Result<ApiResponse, String> {
        let call = {
            let mut urls = lock(&self.urls);
            urls.push(request.url.to_string());
            urls.len() - 1
        };
        tokio::time::sleep(self.delay).await;
        let bodies = lock(&self.bodies);
        let (status, body) = bodies[call.min(bodies.len() - 1)].clone();
        Ok(ApiResponse {
            url: String::new(),
            status,
            headers: BTreeMap::new(),
            body: body.into_bytes(),
        })
    }
}

fn envelope(zsrc: &str) -> Value {
    json!({"code": 0, "data": {"configs": {"zsrc": {"url": zsrc}}}})
}

pub(super) fn service(
    endpoint: EndpointSource,
    transport: Arc<Stub>,
    clock: Arc<AtomicU64>,
    timeout: Duration,
) -> ClientConfigService {
    let api = Arc::new(ApiClient::new(
        endpoint,
        ClientIdentity::default(),
        transport,
    ));
    ClientConfigService::new(api, "3.15.1".into(), "linux-x64".into())
        .with_clock_and_timeout(Arc::new(move || clock.load(Ordering::SeqCst)), timeout)
}

pub(super) fn fixed(origin: &str) -> EndpointSource {
    EndpointSource {
        escode_env: Some("production".into()),
        base_origin: Some(origin.into()),
        settings_home: None,
    }
}

pub(super) const LONG: Duration = Duration::from_secs(5);

#[tokio::test]
async fn snapshots_are_cached_for_an_hour_and_force_refresh_bypasses_the_cache() {
    let stub = Stub::ok(envelope("https://a.example.com"));
    let clock = Arc::new(AtomicU64::new(1_000));
    let config = service(
        fixed("https://api.example.com"),
        Arc::clone(&stub),
        Arc::clone(&clock),
        LONG,
    );

    let first = config.get_snapshot(false).await.unwrap();
    assert_eq!(first.zsrc_url.as_deref(), Some("https://a.example.com"));
    assert_eq!(
        lock(&stub.urls)[0],
        "https://api.example.com/api/v1/client/configs?app_version=3.15.1&platform=linux-x64"
    );
    config.get_snapshot(false).await.unwrap();
    assert_eq!(stub.count(), 1, "TTL 内命中缓存");

    config.get_snapshot(true).await.unwrap();
    assert_eq!(stub.count(), 2, "forceRefresh 绕过缓存");

    clock.store(1_000 + CACHE_TTL_MS - 1, Ordering::SeqCst);
    config.get_snapshot(false).await.unwrap();
    assert_eq!(stub.count(), 2, "TTL 边界前仍命中");
    clock.store(1_000 + CACHE_TTL_MS + 1, Ordering::SeqCst);
    config.get_snapshot(false).await.unwrap();
    assert_eq!(stub.count(), 3, "过期后重新请求");
}

#[tokio::test]
async fn concurrent_callers_share_one_request_and_one_result() {
    let stub = Stub::new(
        vec![(200, envelope("https://a.example.com").to_string())],
        Duration::from_millis(80),
    );
    let config = service(
        fixed("https://api.example.com"),
        Arc::clone(&stub),
        Arc::default(),
        LONG,
    );
    let (a, b, c) = tokio::join!(
        config.get_snapshot(false),
        config.get_snapshot(true),
        config.get_snapshot(false)
    );
    assert_eq!(stub.count(), 1, "forceRefresh 遇到在途请求也加入它");
    assert_eq!(a.unwrap(), b.unwrap());
    assert!(c.is_ok());
}

#[tokio::test]
async fn failures_are_shared_by_waiters_and_never_cached() {
    let stub = Stub::new(
        vec![
            (503, "x".into()),
            (200, envelope("https://a.example.com").to_string()),
        ],
        Duration::from_millis(60),
    );
    let config = service(
        fixed("https://api.example.com"),
        Arc::clone(&stub),
        Arc::default(),
        LONG,
    );
    let (a, b) = tokio::join!(config.get_snapshot(false), config.get_snapshot(false));
    let expected = ConfigError::Plain("Public client config HTTP 503".into());
    assert_eq!(
        (a.unwrap_err(), b.unwrap_err()),
        (expected.clone(), expected)
    );
    assert_eq!(stub.count(), 1);
    // 失败不写缓存：下一次重新请求并成功。
    assert!(config.get_snapshot(false).await.is_ok());
    assert_eq!(stub.count(), 2);
}

#[tokio::test]
async fn bad_payloads_map_to_the_ts_error_classes() {
    let at =
        |stub: Arc<Stub>| service(fixed("https://api.example.com"), stub, Arc::default(), LONG);

    let error = at(Stub::ok(json!({"code": 1})))
        .get_snapshot(false)
        .await
        .unwrap_err();
    assert_eq!(
        (error.name(), error.message()),
        ("Error", "Invalid public client config response")
    );

    let error = at(Stub::new(vec![(200, "<html>".into())], Duration::ZERO))
        .get_snapshot(false)
        .await
        .unwrap_err();
    assert_eq!(error.name(), "SyntaxError");

    let error = at(Stub::new(vec![(404, "{}".into())], Duration::ZERO))
        .get_snapshot(false)
        .await
        .unwrap_err();
    assert_eq!(error.message(), "Public client config HTTP 404");
}

#[tokio::test]
async fn a_slow_endpoint_times_out_with_the_ts_message() {
    let stub = Stub::new(
        vec![(200, envelope("https://a.example.com").to_string())],
        Duration::from_millis(300),
    );
    let config = service(
        fixed("https://api.example.com"),
        stub,
        Arc::default(),
        Duration::from_millis(30),
    );
    let error = config.get_snapshot(false).await.unwrap_err();
    assert_eq!(error.message(), "Public client config request timed out");
}

#[tokio::test]
async fn switching_the_endpoint_uses_a_separate_cache_entry_and_url() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = EndpointSource {
        escode_env: Some("test".into()),
        base_origin: Some("https://one.example.com".into()),
        settings_home: Some(home.path().to_path_buf()),
    };
    let stub = Stub::ok(envelope("https://a.example.com"));
    let config = service(endpoint, Arc::clone(&stub), Arc::default(), LONG);
    config.get_snapshot(false).await.unwrap();
    config.get_snapshot(false).await.unwrap();
    assert_eq!(stub.count(), 1);

    let settings = crate::settings::settings_path(home.path());
    tokio::fs::create_dir_all(settings.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &settings,
        r#"{"escodeEndpointOrigin":"https://two.example.com"}"#,
    )
    .await
    .unwrap();
    config.get_snapshot(false).await.unwrap();
    assert_eq!(stub.count(), 2, "新 Endpoint 不复用旧 Endpoint 的缓存");
    assert!(lock(&stub.urls)[1].starts_with("https://two.example.com/api/v1/client/configs?"));
    // 切回去：旧条目仍在缓存里。
    tokio::fs::write(&settings, "{}").await.unwrap();
    config.get_snapshot(false).await.unwrap();
    assert_eq!(stub.count(), 2);
}

#[test]
fn read_options_follow_client_config_read_options_schema() {
    let ok = |value: Value| parse_read_options(Some(&value));
    assert_eq!(parse_read_options(None), Ok(false));
    assert_eq!(ok(json!({})), Ok(false));
    assert_eq!(ok(json!({"forceRefresh": true})), Ok(true));
    assert_eq!(
        ok(json!({"forceRefresh": false, "extra": 1})),
        Ok(false),
        "未知键被忽略"
    );
    for bad in [
        json!(null),
        json!([]),
        json!("x"),
        json!(1),
        json!({"forceRefresh": "yes"}),
        json!({"forceRefresh": null}),
        json!({"forceRefresh": 1}),
    ] {
        assert_eq!(ok(bad.clone()).unwrap_err().name(), "ZodError", "{bad}");
    }
}
