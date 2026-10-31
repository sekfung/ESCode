//! Provider Provisioning（S4-g，docs/specs/source-server-remote-rust-host.md §11.7）。
//!
//! Owner 只活在 Host 进程内：读取本地 Personal Provider Config、账号窄设置与 allowlist 凭据，
//! 组 `ProviderProvisioningEnvelope(schemaVersion=2)` 后调用远端
//! `provider-provisioning-target.apply`。信封只存在于内存，不写日志、不进 Renderer。
//!
//! 连接事实仍由 `SourceServerConnections` 唯一拥有；本模块只维护 per-profile 的单飞 lane，
//! 在首连屏障时等待一次同步，后续触发只推进 generation 并合并尾随同步。

#[path = "source_server_provider_provisioning_peer.rs"]
mod peer;
#[path = "source_server_provider_provisioning_source.rs"]
mod source;

pub use peer::{
    PROVIDER_PROVISIONING_TARGET_CHANNEL, ProviderProvisioningPeer, RemoteProvisioningPeer,
};
pub use source::LocalProviderProvisioningSource;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::anyhow;
use escode_host_control::logging;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

/// 与 TS `PROVIDER_PROVISIONING_OAUTH_CREDENTIAL_KEYS` 完全一致；顺序即文件读取顺序。
pub const PROVIDER_PROVISIONING_OAUTH_CREDENTIAL_KEYS: &[&str] = &[
    "oauth:active_provider",
    "oauth:zai:access_token",
    "oauth:zai:refresh_token",
    "oauth:zai:user_info",
    "oauth:bigmodel:access_token",
    "oauth:bigmodel:refresh_token",
    "oauth:bigmodel:user_info",
    "escodejwttoken",
];

/// 单次远端 apply 的上限：target 会写文件并刷新 Registry，但不应无限挂起连接。
pub const PROVIDER_PROVISIONING_TIMEOUT: Duration = Duration::from_secs(30);

pub fn is_provisioning_credential_key(key: &str) -> bool {
    PROVIDER_PROVISIONING_OAUTH_CREDENTIAL_KEYS.contains(&key)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisioningTrigger {
    EnvironmentOnline,
    PersonalConfig,
    ConfiguredDefault,
    AccountSettings,
    Credential,
}

impl ProvisioningTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvironmentOnline => "environment-online",
            Self::PersonalConfig => "personal-config",
            Self::ConfiguredDefault => "configured-default",
            Self::AccountSettings => "account-settings",
            Self::Credential => "credential",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisioningStatus {
    Applied,
    AlreadyApplied,
    Unsupported,
    Failed,
    RollbackFailed,
}

impl ProvisioningStatus {
    fn from_remote(status: &str) -> Option<Self> {
        match status {
            "applied" => Some(Self::Applied),
            "already-applied" => Some(Self::AlreadyApplied),
            "unsupported" => Some(Self::Unsupported),
            "failed" => Some(Self::Failed),
            "rollback_failed" => Some(Self::RollbackFailed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::AlreadyApplied => "already-applied",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::RollbackFailed => "rollback_failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisioningOutcome {
    pub sync_id: String,
    pub status: ProvisioningStatus,
    pub personal_provider_count: u64,
    pub credential_count: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// 首连屏障失败：HTTP/RPC 层据此回传稳定 errorCode，而不是解析错误文案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderProvisioningFailure {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for ProviderProvisioningFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderProvisioningFailure {}

impl ProvisioningOutcome {
    pub fn allows_online(&self) -> bool {
        matches!(
            self.status,
            ProvisioningStatus::Applied | ProvisioningStatus::AlreadyApplied
        )
    }

    pub fn failure_code(&self) -> &'static str {
        match self.status {
            ProvisioningStatus::Unsupported => "provider-provisioning-unsupported",
            _ => "provider-provisioning-failed",
        }
    }

    pub fn failure(&self) -> anyhow::Error {
        anyhow!(
            "provider provisioning {}: {}",
            self.status.as_str(),
            self.error_code.as_deref().unwrap_or(self.failure_code())
        )
    }
}

fn failed_outcome(
    sync_id: String,
    error_code: &str,
    message: impl Into<String>,
) -> ProvisioningOutcome {
    ProvisioningOutcome {
        sync_id,
        status: ProvisioningStatus::Failed,
        personal_provider_count: 0,
        credential_count: 0,
        error_code: Some(error_code.to_owned()),
        error_message: Some(message.into()),
    }
}

fn parse_outcome(sync_id: String, value: Value) -> ProvisioningOutcome {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .and_then(ProvisioningStatus::from_remote);
    let Some(status) = status else {
        return failed_outcome(
            sync_id,
            "target-apply-failed",
            "provider-provisioning-target returned an unknown status",
        );
    };
    let count = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    let error_code = value
        .get("errorCode")
        .and_then(Value::as_str)
        .filter(|code| PROVISIONING_ERROR_CODES.contains(code))
        .map(str::to_owned)
        .or_else(|| match status {
            ProvisioningStatus::Failed
            | ProvisioningStatus::RollbackFailed
            | ProvisioningStatus::Unsupported => Some("target-apply-failed".to_owned()),
            _ => None,
        });
    ProvisioningOutcome {
        sync_id,
        status,
        personal_provider_count: count("personalProviderCount"),
        credential_count: count("credentialCount"),
        error_code,
        error_message: value
            .get("errorMessage")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

/// 只透传固定阶段码，避免把远端自由文本当成可公开诊断信息（与 TS executor 一致）。
const PROVISIONING_ERROR_CODES: &[&str] = &[
    "source-read-failed",
    "target-call-failed",
    "target-write-failed",
    "target-refresh-failed",
    "target-commit-failed",
    "target-apply-failed",
    "capability-unavailable",
    "session-unavailable",
    "host-execution-failed",
];

#[async_trait::async_trait]
pub trait ProviderProvisioningSource: Send + Sync {
    async fn read(&self, sync_id: &str) -> anyhow::Result<Value>;
}

struct Lane {
    token: u64,
    generation: u64,
    running: bool,
    serial: Arc<AsyncMutex<()>>,
}

pub struct SourceServerProviderProvisioning {
    source: Arc<dyn ProviderProvisioningSource>,
    timeout: Duration,
    lanes: Mutex<BTreeMap<String, Lane>>,
}

impl SourceServerProviderProvisioning {
    pub fn new(source: Arc<dyn ProviderProvisioningSource>, timeout: Duration) -> Self {
        Self {
            source,
            timeout,
            lanes: Mutex::new(BTreeMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Lane>> {
        self.lanes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ensure_lane(&self, profile_id: &str, attempt: u64) -> Arc<AsyncMutex<()>> {
        let mut lanes = self.lock();
        let lane = lanes.entry(profile_id.to_owned()).or_insert_with(|| Lane {
            token: attempt,
            generation: 0,
            running: false,
            serial: Arc::new(AsyncMutex::new(())),
        });
        if lane.token != attempt {
            lane.token = attempt;
            lane.generation = 0;
            lane.running = false;
            lane.serial = Arc::new(AsyncMutex::new(()));
        }
        Arc::clone(&lane.serial)
    }

    /// 连接收口/换代时移除 lane；在途任务通过 token 比对自行失效。
    pub fn remove(&self, profile_id: &str) {
        self.lock().remove(profile_id);
    }

    pub fn dispose(&self) {
        self.lock().clear();
    }

    /// 首连屏障：等待一次完整同步，并返回结果供连接管理器决定是否发布 online。
    pub async fn sync(
        &self,
        profile_id: &str,
        attempt: u64,
        peer: Arc<dyn ProviderProvisioningPeer>,
        _trigger: ProvisioningTrigger,
    ) -> ProvisioningOutcome {
        let serial = self.ensure_lane(profile_id, attempt);
        let _guard = Arc::clone(&serial).lock_owned().await;
        self.execute(peer).await
    }

    /// 后续触发：同一 profile 同时最多一个同步；在途触发只合并为一次尾随同步。
    pub fn request(
        self: &Arc<Self>,
        profile_id: &str,
        attempt: u64,
        peer: Arc<dyn ProviderProvisioningPeer>,
        trigger: ProvisioningTrigger,
    ) {
        let serial = self.ensure_lane(profile_id, attempt);
        let start = {
            let mut lanes = self.lock();
            let Some(lane) = lanes.get_mut(profile_id) else {
                return;
            };
            lane.generation = lane.generation.wrapping_add(1);
            if lane.running {
                false
            } else {
                lane.running = true;
                true
            }
        };
        if !start {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            if let Some(lane) = self.lock().get_mut(profile_id) {
                lane.running = false;
            }
            return;
        };
        let owner = Arc::clone(self);
        let profile = profile_id.to_owned();
        handle.spawn(async move {
            loop {
                let generation = owner.lock().get(&profile).map_or(0, |lane| lane.generation);
                let guard = Arc::clone(&serial).lock_owned().await;
                let outcome = owner.execute(Arc::clone(&peer)).await;
                if !outcome.allows_online() {
                    owner.log_warning(&profile, trigger, &outcome);
                }
                drop(guard);
                let repeat = {
                    let mut lanes = owner.lock();
                    match lanes.get_mut(&profile) {
                        Some(lane) if lane.token != attempt => {
                            lane.running = false;
                            false
                        }
                        Some(lane) if lane.generation != generation => true,
                        Some(lane) => {
                            lane.running = false;
                            false
                        }
                        None => false,
                    }
                };
                if !repeat {
                    break;
                }
            }
        });
    }

    async fn execute(&self, peer: Arc<dyn ProviderProvisioningPeer>) -> ProvisioningOutcome {
        let sync_id = uuid::Uuid::new_v4().to_string();
        let envelope = match self.source.read(&sync_id).await {
            Ok(envelope) => envelope,
            Err(error) => {
                return failed_outcome(sync_id, "source-read-failed", format!("{error:#}"));
            }
        };
        if !peer.supports_target() {
            return ProvisioningOutcome {
                sync_id,
                status: ProvisioningStatus::Unsupported,
                personal_provider_count: 0,
                credential_count: 0,
                error_code: Some("capability-unavailable".to_owned()),
                error_message: Some(
                    "remote source server has no provider-provisioning-target".to_owned(),
                ),
            };
        }
        match tokio::time::timeout(self.timeout, peer.apply(envelope)).await {
            Ok(Ok(value)) => parse_outcome(sync_id, value),
            Ok(Err(error)) => failed_outcome(sync_id, "target-call-failed", format!("{error:#}")),
            Err(_) => failed_outcome(
                sync_id,
                "target-call-failed",
                format!("provider provisioning timed out after {:?}", self.timeout),
            ),
        }
    }

    pub fn log_warning(
        &self,
        profile_id: &str,
        trigger: ProvisioningTrigger,
        outcome: &ProvisioningOutcome,
    ) {
        // 只记录固定阶段信息与 syncId；Envelope / 凭据值绝不进入日志。
        logging::warn(
            "source-server",
            format!(
                "provider provisioning {} for {profile_id} ended {} (code={}, syncId={})",
                trigger.as_str(),
                outcome.status.as_str(),
                outcome.error_code.as_deref().unwrap_or("-"),
                outcome.sync_id
            ),
        );
    }
}

#[cfg(test)]
#[path = "source_server_provider_provisioning_source_tests.rs"]
mod source_tests;

#[cfg(test)]
#[path = "source_server_provider_provisioning_tests.rs"]
mod tests;
