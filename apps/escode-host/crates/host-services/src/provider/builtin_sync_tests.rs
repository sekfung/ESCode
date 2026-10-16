use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::{Value, json};
use url::Url;

use super::super::builtin_source::{ActiveCache, active_file_path, client_platform};
use super::super::endpoint::EndpointSource;
use super::super::service_tests::builtin;
use super::*;

const ORIGIN: &str = "https://api.example.com";

fn release(revision: u64, tag: &str) -> Value {
    let mut value = builtin();
    value["revision"] = json!(revision);
    value["config"]["providerConfigRules"]["providerRules"][0]["providerId"] = json!(tag);
    value
}

fn client_config(has_release: bool) -> Value {
    let mut configs = json!({});
    if has_release {
        configs["builtin_provider_config_json"] = json!("https://cdn.example.com/rel.json");
    }
    json!({"code": 0, "data": {"configs": configs}})
}

/// 回放桩：`config` 回 client-config，`release` 回 Release；每次请求前执行 `hook`。
struct Stub {
    config: Result<Value, String>,
    release: Result<Value, String>,
    requests: AtomicU64,
    hook: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Stub {
    fn ok(release: Value) -> Arc<Self> {
        Arc::new(Self {
            config: Ok(client_config(true)),
            release: Ok(release),
            requests: AtomicU64::new(0),
            hook: Mutex::default(),
        })
    }
    fn failing() -> Arc<Self> {
        Arc::new(Self {
            config: Err("HTTP 500".into()),
            release: Err("unused".into()),
            requests: AtomicU64::new(0),
            hook: Mutex::default(),
        })
    }
    fn count(&self) -> u64 {
        self.requests.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JsonGet for Stub {
    async fn get_json(&self, url: &Url) -> Result<Value, String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = &*self.hook.lock().unwrap() {
            hook();
        }
        if url.path() == "/rel.json" {
            self.release.clone()
        } else {
            self.config.clone()
        }
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    clock: Arc<AtomicU64>,
    source: Arc<BuiltinSource>,
}

impl Fixture {
    async fn new(bundled_revision: u64) -> Self {
        Self::with_endpoint(
            bundled_revision,
            EndpointSource {
                escode_env: Some("production".into()),
                base_origin: Some(ORIGIN.into()),
                settings_home: None,
            },
        )
        .await
    }

    async fn with_endpoint(bundled_revision: u64, endpoint: EndpointSource) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("bundled.json");
        tokio::fs::write(&bundled, release(bundled_revision, "b").to_string())
            .await
            .unwrap();
        let cache = ActiveCache {
            root: dir.path().join("cache"),
            app_version: "1.2.3".into(),
            endpoint,
        };
        Self {
            clock: Arc::new(AtomicU64::new(1_000_000)),
            source: Arc::new(BuiltinSource::new(bundled, Some(cache))),
            dir,
        }
    }

    fn sync(&self, stub: &Arc<Stub>) -> Synchronizer {
        let clock = Arc::clone(&self.clock);
        Synchronizer::new(
            Arc::clone(&self.source),
            Arc::clone(stub) as Arc<dyn JsonGet>,
        )
        .with_clock(Arc::new(move || clock.load(Ordering::SeqCst)))
    }

    fn active(&self, origin: &str) -> PathBuf {
        active_file_path(
            &self.dir.path().join("cache"),
            "1.2.3",
            origin,
            &client_platform(),
        )
        .unwrap()
    }

    async fn control(&self) -> Value {
        let path = control_file_path(&self.active(ORIGIN));
        serde_json::from_str(&tokio::fs::read_to_string(path).await.unwrap()).unwrap()
    }

    async fn write_control(&self, control: Value) {
        let path = control_file_path(&self.active(ORIGIN));
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, control.to_string()).await.unwrap();
    }
}

#[tokio::test]
async fn a_newer_release_is_applied_and_the_control_file_records_success() {
    let fixture = Fixture::new(1).await;
    let stub = Stub::ok(release(5, "remote"));
    assert_eq!(
        fixture.sync(&stub).refresh(false).await,
        Ok(RefreshResult::Updated)
    );
    let active: Value = serde_json::from_str(
        &tokio::fs::read_to_string(fixture.active(ORIGIN))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active, release(5, "remote"));
    assert_eq!(
        fixture.control().await,
        json!({"schemaVersion": 1, "endpointKey": ORIGIN, "leaseUntil": 0,
               "nextEligibleAt": 1_000_000 + 3_600_000, "failureCount": 0})
    );
}

#[tokio::test]
async fn not_due_skips_without_network_and_force_overrides_it() {
    let fixture = Fixture::new(1).await;
    let stub = Stub::ok(release(5, "remote"));
    let sync = fixture.sync(&stub);
    assert_eq!(sync.refresh(false).await, Ok(RefreshResult::Updated));
    let after_first = stub.count();
    assert_eq!(
        sync.refresh(false).await,
        Ok(RefreshResult::Skipped(Skip::NotDue))
    );
    assert_eq!(stub.count(), after_first, "未到期不应触网");
    assert_eq!(sync.refresh(true).await, Ok(RefreshResult::Unchanged));
    assert!(stub.count() > after_first);
}

#[tokio::test]
async fn a_held_lease_blocks_even_a_forced_refresh() {
    let fixture = Fixture::new(1).await;
    fixture
        .write_control(
            json!({"schemaVersion": 1, "endpointKey": ORIGIN, "leaseId": "other",
            "leaseUntil": 1_000_000 + 10_000, "nextEligibleAt": 0, "failureCount": 0}),
        )
        .await;
    let stub = Stub::ok(release(5, "remote"));
    assert_eq!(
        fixture.sync(&stub).refresh(true).await,
        Ok(RefreshResult::Skipped(Skip::LeaseHeld))
    );
    assert_eq!(stub.count(), 0);
    // 租约过期后可以接管。
    fixture.clock.store(1_000_000 + 10_001, Ordering::SeqCst);
    assert_eq!(
        fixture.sync(&stub).refresh(true).await,
        Ok(RefreshResult::Updated)
    );
}

#[tokio::test]
async fn another_endpoints_control_state_does_not_throttle_this_one() {
    let fixture = Fixture::new(1).await;
    fixture
        .write_control(
            json!({"schemaVersion": 1, "endpointKey": "https://other.example.com",
            "leaseUntil": 9_999_999, "nextEligibleAt": 9_999_999, "failureCount": 7}),
        )
        .await;
    let stub = Stub::ok(release(5, "remote"));
    assert_eq!(
        fixture.sync(&stub).refresh(false).await,
        Ok(RefreshResult::Updated)
    );
    assert_eq!(fixture.control().await["failureCount"], 0);
}

#[tokio::test]
async fn a_corrupt_control_file_is_rebuilt() {
    let fixture = Fixture::new(1).await;
    let path = control_file_path(&fixture.active(ORIGIN));
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, "{broken").await.unwrap();
    let stub = Stub::ok(release(5, "remote"));
    assert_eq!(
        fixture.sync(&stub).refresh(false).await,
        Ok(RefreshResult::Updated)
    );
}

#[tokio::test]
async fn revisions_only_move_forward() {
    let fixture = Fixture::new(9).await;
    let stale = Stub::ok(release(3, "old"));
    assert_eq!(
        fixture.sync(&stale).refresh(true).await,
        Ok(RefreshResult::Stale)
    );
    // Active 仍是被物化的 Bundled。
    let active: Value = serde_json::from_str(
        &tokio::fs::read_to_string(fixture.active(ORIGIN))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active["revision"], 9);
    let conflict = Stub::ok(release(9, "different"));
    let error = fixture.sync(&conflict).refresh(true).await.unwrap_err();
    assert!(error.contains("相同 revision 9 对应不同内容"), "{error}");
    assert_eq!(fixture.control().await["failureCount"], 1);
}

#[tokio::test]
async fn failures_back_off_exponentially_up_to_the_cap() {
    let fixture = Fixture::new(1).await;
    let stub = Stub::failing();
    let sync = fixture.sync(&stub);
    for (attempt, delay) in [(1, 60_000u64), (2, 120_000), (3, 240_000)] {
        let error = sync.refresh(true).await.unwrap_err();
        assert_eq!(error, "ESCode Built-in client-config: HTTP 500");
        let control = fixture.control().await;
        assert_eq!(control["failureCount"], attempt);
        assert_eq!(
            control["nextEligibleAt"],
            1_000_000 + delay,
            "attempt {attempt}"
        );
    }
    fixture
        .write_control(json!({"schemaVersion": 1, "endpointKey": ORIGIN,
            "leaseUntil": 0, "nextEligibleAt": 0, "failureCount": 20}))
        .await;
    sync.refresh(true).await.unwrap_err();
    assert_eq!(
        fixture.control().await["nextEligibleAt"],
        1_000_000 + 3_600_000
    );
}

#[tokio::test]
async fn a_missing_release_is_success_not_failure() {
    let fixture = Fixture::new(1).await;
    let stub = Arc::new(Stub {
        config: Ok(client_config(false)),
        release: Err("unused".into()),
        requests: AtomicU64::new(0),
        hook: Mutex::default(),
    });
    assert_eq!(
        fixture.sync(&stub).refresh(false).await,
        Ok(RefreshResult::Missing)
    );
    assert_eq!(fixture.control().await["failureCount"], 0);
}

#[tokio::test]
async fn finishing_never_overwrites_a_lease_taken_over_by_someone_else() {
    let fixture = Fixture::new(1).await;
    let stub = Stub::ok(release(5, "remote"));
    let control_path = control_file_path(&fixture.active(ORIGIN));
    let takeover = json!({"schemaVersion": 1, "endpointKey": ORIGIN, "leaseId": "someone-else",
        "leaseUntil": 5_000_000, "nextEligibleAt": 0, "failureCount": 0});
    *stub.hook.lock().unwrap() = Some(Box::new({
        let (path, takeover) = (control_path.clone(), takeover.to_string());
        move || std::fs::write(&path, &takeover).unwrap()
    }));
    assert_eq!(
        fixture.sync(&stub).refresh(false).await,
        Ok(RefreshResult::Updated)
    );
    assert_eq!(fixture.control().await, takeover);
}

#[tokio::test]
async fn an_endpoint_switched_during_the_download_discards_the_result() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = EndpointSource {
        escode_env: Some("test".into()),
        base_origin: Some(ORIGIN.into()),
        settings_home: Some(home.path().to_path_buf()),
    };
    let fixture = Fixture::with_endpoint(1, endpoint).await;
    let stub = Stub::ok(release(5, "remote"));
    let settings = crate::settings::settings_path(home.path());
    *stub.hook.lock().unwrap() = Some(Box::new(move || {
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(
            &settings,
            r#"{"escodeEndpointOrigin":"https://switched.example.com"}"#,
        )
        .unwrap();
    }));
    assert_eq!(
        fixture.sync(&stub).refresh(false).await,
        Ok(RefreshResult::Skipped(Skip::EndpointChanged))
    );
    // 下载结果属于旧 Endpoint：任何一个 Endpoint 的 Active 都不应被写入。
    assert!(
        !fixture.active(ORIGIN).exists(),
        "旧 Endpoint 的 Release 不能落到缓存"
    );
    assert!(!fixture.active("https://switched.example.com").exists());
}

#[tokio::test]
async fn the_service_picks_up_a_remote_release_through_the_scheduled_refresh() {
    use super::super::ProviderService;
    let fixture = Fixture::new(1).await;
    let personal = fixture.dir.path().join("personal.json");
    let cache = ActiveCache {
        root: fixture.dir.path().join("cache"),
        app_version: "1.2.3".into(),
        endpoint: EndpointSource {
            escode_env: Some("production".into()),
            base_origin: Some(ORIGIN.into()),
            settings_home: None,
        },
    };
    let stub = Stub::ok(release(5, "remote"));
    let service = Arc::new(
        ProviderService::new(fixture.dir.path().join("bundled.json"), personal)
            .with_active_cache(cache)
            .with_remote_refresh(Arc::clone(&stub) as Arc<dyn JsonGet>),
    );
    let revision = |view: Value| view["revision"].as_u64().unwrap();
    let before = revision(service.model_selection_view(None).await.unwrap());
    service.start_remote_refresh();
    service.start_remote_refresh(); // 幂等：不会起第二个循环
    let mut after = before;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        after = revision(service.model_selection_view(None).await.unwrap());
        if after > before {
            break;
        }
    }
    assert!(
        after > before,
        "远端 Release 写入 Active 后 Registry 修订号应前进"
    );
    assert_eq!(
        stub.count(),
        2,
        "一次刷新 = client-config + cdn 两次请求，且只刷新一次"
    );
}
