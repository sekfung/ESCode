//! 动态工作流灰度的服务层行为（spec §4 / §5 S3）。纯函数折叠规则由 `snapshot_tests` 里的 TS 语料对拍。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::service::{CACHE_TTL_MS, ClientConfigService, lock};
use super::service_tests::{LONG, Stub, fixed, service};
use crate::provider::EndpointSource;

fn dynamic_envelope(mode: &str) -> Value {
    json!({"code": 0, "data": {"configs": {"dynamicWorkflow": {"mode": mode}}}})
}

fn with_dynamic(
    config: ClientConfigService,
    env_override: Option<&str>,
    home: Option<&std::path::Path>,
) -> ClientConfigService {
    config.with_dynamic_workflow(
        env_override.map(str::to_owned),
        home.map(std::path::Path::to_path_buf),
    )
}

async fn write_user_mode(home: &std::path::Path, mode: &str) {
    let settings = crate::settings::settings_path(home);
    tokio::fs::create_dir_all(settings.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&settings, json!({"dynamicWorkflowMode": mode}).to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_valid_env_override_short_circuits_without_a_request() {
    let stub = Stub::ok(dynamic_envelope("alwaysOn"));
    let config = with_dynamic(
        service(
            fixed("https://api.example.com"),
            Arc::clone(&stub),
            Arc::default(),
            LONG,
        ),
        Some(" onDemand "),
        None,
    );
    let result = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(result["mode"], "onDemand");
    assert_eq!(result["source"], "override");
    assert_eq!(stub.count(), 0, "覆盖合法即短路");
}

#[tokio::test]
async fn the_remote_mode_is_fetched_once_cached_and_force_refresh_refetches() {
    let stub = Stub::new(
        vec![
            (200, dynamic_envelope("onDemand").to_string()),
            (200, dynamic_envelope("alwaysOn").to_string()),
        ],
        Duration::ZERO,
    );
    let clock = Arc::new(AtomicU64::new(10));
    let config = with_dynamic(
        service(
            fixed("https://api.example.com"),
            Arc::clone(&stub),
            Arc::clone(&clock),
            LONG,
        ),
        None,
        None,
    );
    let first = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(
        (first["mode"].as_str(), first["source"].as_str()),
        (Some("onDemand"), Some("remote"))
    );
    config.get_dynamic_workflow(false).await;
    assert_eq!(stub.count(), 1, "原始信封缓存 1 h");
    let refreshed = config.get_dynamic_workflow(true).await.to_json();
    assert_eq!(refreshed["mode"], "alwaysOn");
    assert_eq!(stub.count(), 2);
    clock.store(10 + CACHE_TTL_MS + 1, Ordering::SeqCst);
    config.get_dynamic_workflow(false).await;
    assert_eq!(stub.count(), 3, "过期后重新请求");
}

#[tokio::test]
async fn request_failures_fail_closed_and_are_not_cached() {
    let stub = Stub::new(
        vec![
            (500, "x".into()),
            (200, dynamic_envelope("alwaysOn").to_string()),
        ],
        Duration::ZERO,
    );
    let config = with_dynamic(
        service(
            fixed("https://api.example.com"),
            Arc::clone(&stub),
            Arc::default(),
            LONG,
        ),
        None,
        None,
    );
    let failed = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(
        (
            failed["mode"].as_str(),
            failed["source"].as_str(),
            failed["enabled"].as_bool()
        ),
        (Some("disabled"), Some("default"), Some(false))
    );
    let recovered = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(
        recovered["mode"], "alwaysOn",
        "失败不写缓存，下一次重新请求"
    );
    assert_eq!(stub.count(), 2);
}

#[tokio::test]
async fn the_users_choice_is_read_from_settings_on_every_call() {
    let home = tempfile::tempdir().unwrap();
    let stub = Stub::ok(dynamic_envelope("onDemand"));
    let config = with_dynamic(
        service(
            fixed("https://api.example.com"),
            Arc::clone(&stub),
            Arc::default(),
            LONG,
        ),
        None,
        Some(home.path()),
    );
    let followed = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(followed.get("userMode"), None, "没有选择 = 跟随提供值");
    write_user_mode(home.path(), "alwaysOn").await;
    let chosen = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(
        (chosen["mode"].as_str(), chosen["userMode"].as_str()),
        (Some("alwaysOn"), Some("alwaysOn"))
    );
    assert_eq!(chosen["offeredMode"], "onDemand", "提供值不变");
    assert_eq!(stub.count(), 1, "用户选择变化不触发网络");
    write_user_mode(home.path(), "bogus").await;
    assert_eq!(
        config
            .get_dynamic_workflow(false)
            .await
            .to_json()
            .get("userMode"),
        None
    );
}

#[tokio::test]
async fn a_user_choice_can_never_enable_a_feature_the_server_keeps_off() {
    let home = tempfile::tempdir().unwrap();
    write_user_mode(home.path(), "alwaysOn").await;
    let stub = Stub::ok(dynamic_envelope("disabled"));
    let config = with_dynamic(
        service(fixed("https://api.example.com"), stub, Arc::default(), LONG),
        None,
        Some(home.path()),
    );
    let result = config.get_dynamic_workflow(false).await.to_json();
    assert_eq!(
        (result["mode"].as_str(), result["enabled"].as_bool()),
        (Some("disabled"), Some(false))
    );
    assert_eq!(result.get("userMode"), None);
}

#[tokio::test]
async fn the_raw_envelope_cache_is_not_keyed_by_endpoint_like_ts() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = EndpointSource {
        escode_env: Some("test".into()),
        base_origin: Some("https://one.example.com".into()),
        settings_home: Some(home.path().to_path_buf()),
    };
    let stub = Stub::ok(dynamic_envelope("onDemand"));
    let config = with_dynamic(
        service(endpoint, Arc::clone(&stub), Arc::default(), LONG),
        None,
        None,
    );
    config.get_dynamic_workflow(false).await;
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
    config.get_dynamic_workflow(false).await;
    // 登记差异之外的行为一致：TS 的原始信封缓存只有一条，切换 Endpoint 不会重新请求（直到 TTL/forceRefresh）。
    assert_eq!(stub.count(), 1);
    config.get_dynamic_workflow(true).await;
    assert!(lock(&stub.urls)[1].starts_with("https://two.example.com/"));
}
