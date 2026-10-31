//! S4-g Provider Provisioning owner：首连结果、固定错误码与触发合并。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::*;

#[derive(Default)]
struct FakeSource {
    reads: AtomicUsize,
    fail: bool,
}

#[async_trait::async_trait]
impl ProviderProvisioningSource for FakeSource {
    async fn read(&self, sync_id: &str) -> Result<Value> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            bail!("source unavailable");
        }
        Ok(json!({"syncId": sync_id}))
    }
}

struct FakePeer {
    supported: bool,
    fail: bool,
    calls: AtomicUsize,
    gate: Option<Arc<Notify>>,
    open: Option<Arc<AtomicBool>>,
    entered: Option<Arc<Notify>>,
}

impl FakePeer {
    fn applied() -> Self {
        Self {
            supported: true,
            fail: false,
            calls: AtomicUsize::new(0),
            gate: None,
            open: None,
            entered: None,
        }
    }
}

#[async_trait::async_trait]
impl ProviderProvisioningPeer for FakePeer {
    fn supports_target(&self) -> bool {
        self.supported
    }

    async fn apply(&self, envelope: Value) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = &self.entered {
            entered.notify_one();
        }
        if let Some(gate) = &self.gate {
            while !self
                .open
                .as_ref()
                .is_some_and(|open| open.load(Ordering::SeqCst))
            {
                gate.notified().await;
            }
        }
        if self.fail {
            bail!("target call failed");
        }
        Ok(json!({
            "syncId": envelope["syncId"],
            "status": "applied",
            "personalProviderCount": 1,
            "credentialCount": 2,
            "rolledBack": false
        }))
    }
}

fn make_owner(source: Arc<FakeSource>, timeout: Duration) -> Arc<SourceServerProviderProvisioning> {
    Arc::new(SourceServerProviderProvisioning::new(source, timeout))
}

#[tokio::test]
async fn applied_and_already_applied_allow_online() {
    let source = Arc::new(FakeSource::default());
    let owner = make_owner(Arc::clone(&source), Duration::from_secs(1));
    let peer = Arc::new(FakePeer::applied());
    let outcome = owner
        .sync("p", 1, peer.clone(), ProvisioningTrigger::EnvironmentOnline)
        .await;
    assert!(outcome.allows_online());
    assert_eq!(outcome.personal_provider_count, 1);
    assert_eq!(outcome.credential_count, 2);
    assert_eq!(peer.calls.load(Ordering::SeqCst), 1);

    // 目标端幂等：already-applied 与 applied 同语义。
    struct AlreadyApplied;
    #[async_trait::async_trait]
    impl ProviderProvisioningPeer for AlreadyApplied {
        fn supports_target(&self) -> bool {
            true
        }
        async fn apply(&self, envelope: Value) -> Result<Value> {
            Ok(json!({
                "syncId": envelope["syncId"],
                "status": "already-applied",
                "personalProviderCount": 0,
                "credentialCount": 0,
                "rolledBack": false
            }))
        }
    }
    let outcome = owner
        .sync(
            "p",
            1,
            Arc::new(AlreadyApplied),
            ProvisioningTrigger::EnvironmentOnline,
        )
        .await;
    assert!(outcome.allows_online());
    assert_eq!(outcome.status, ProvisioningStatus::AlreadyApplied);
}

#[tokio::test]
async fn missing_target_channel_is_unsupported_without_calling_apply() {
    let source = Arc::new(FakeSource::default());
    let owner = make_owner(source, Duration::from_secs(1));
    let peer = Arc::new(FakePeer {
        supported: false,
        ..FakePeer::applied()
    });
    let outcome = owner
        .sync("p", 1, peer.clone(), ProvisioningTrigger::EnvironmentOnline)
        .await;
    assert_eq!(outcome.status, ProvisioningStatus::Unsupported);
    assert_eq!(outcome.failure_code(), "provider-provisioning-unsupported");
    assert_eq!(peer.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn source_and_target_failures_use_fixed_codes() {
    let source = Arc::new(FakeSource {
        fail: true,
        ..FakeSource::default()
    });
    let owner = make_owner(source, Duration::from_secs(1));
    let outcome = owner
        .sync(
            "p",
            1,
            Arc::new(FakePeer::applied()),
            ProvisioningTrigger::EnvironmentOnline,
        )
        .await;
    assert_eq!(outcome.error_code.as_deref(), Some("source-read-failed"));
    assert_eq!(outcome.failure_code(), "provider-provisioning-failed");

    let source = Arc::new(FakeSource::default());
    let owner = make_owner(source, Duration::from_secs(1));
    let peer = Arc::new(FakePeer {
        fail: true,
        ..FakePeer::applied()
    });
    let outcome = owner
        .sync("p", 1, peer, ProvisioningTrigger::EnvironmentOnline)
        .await;
    assert_eq!(outcome.error_code.as_deref(), Some("target-call-failed"));
    assert!(!outcome.allows_online());
}

#[tokio::test]
async fn provisioning_timeout_is_explicit() {
    let source = Arc::new(FakeSource::default());
    let owner = make_owner(source, Duration::from_millis(10));
    let peer = Arc::new(FakePeer {
        gate: Some(Arc::new(Notify::new())),
        ..FakePeer::applied()
    });
    let outcome = owner
        .sync("p", 1, peer, ProvisioningTrigger::EnvironmentOnline)
        .await;
    assert_eq!(outcome.error_code.as_deref(), Some("target-call-failed"));
    assert!(
        outcome
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("timed out"))
    );
}

#[tokio::test]
async fn in_flight_triggers_coalesce_into_one_trailing_sync() {
    let source = Arc::new(FakeSource::default());
    let owner = make_owner(Arc::clone(&source), Duration::from_secs(1));
    let gate = Arc::new(Notify::new());
    let open = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Notify::new());
    let peer = Arc::new(FakePeer {
        gate: Some(Arc::clone(&gate)),
        open: Some(Arc::clone(&open)),
        entered: Some(Arc::clone(&entered)),
        ..FakePeer::applied()
    });

    owner.request("p", 1, peer.clone(), ProvisioningTrigger::PersonalConfig);
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .expect("first apply entered");
    // 在第一次 apply 挂起时到达的两次触发只能形成一次尾随同步。
    owner.request("p", 1, peer.clone(), ProvisioningTrigger::AccountSettings);
    owner.request("p", 1, peer.clone(), ProvisioningTrigger::Credential);
    open.store(true, Ordering::SeqCst);
    gate.notify_one();

    let ok = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if peer.calls.load(Ordering::SeqCst) >= 2 {
                break true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("trailing sync");
    assert!(ok);
    // 尾随同步完成后再给一次调度窗口，确认没有多余的排队同步。
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(peer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(source.reads.load(Ordering::SeqCst), 2);
}
