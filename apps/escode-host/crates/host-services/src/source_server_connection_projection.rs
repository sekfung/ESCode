//! 连接状态的 S4 投影（docs/specs/source-server-remote-rust-host.md §11.1/§11.4）。
//!
//! 只把「已握手事实 + 兼容判定」投影成 `ConnectionStateSnapshot`，不做 IO、不写 registry；
//! `SourceServerConnections` 负责发布与日志。

use crate::source_server::{ConnectionProfile, ConnectionStateSnapshot};
use crate::source_server_compatibility::{
    CompatibilityEvaluation, CompatibilityManifest, UpgradeTarget,
};

pub(crate) fn online_state(
    profile: &ConnectionProfile,
    version: &str,
    compatibility: Option<&CompatibilityEvaluation>,
    local: &CompatibilityManifest,
    updated_at: u64,
) -> ConnectionStateSnapshot {
    let mut state = ConnectionStateSnapshot::new(
        &profile.profile_id,
        profile.source_server_id.as_deref(),
        "online",
        updated_at,
    );
    state.current_version = Some(version.to_owned());
    if let Some(evaluation) = compatibility {
        state.update_availability = Some(if evaluation.update_available {
            "available"
        } else {
            "none"
        });
        if evaluation.update_available {
            state.available_version = Some(local.product_version.clone());
        }
        state.minimum_version = Some(evaluation.required_minimum_version.clone());
    }
    state
}

/// 远端较旧 → `needs-upgrade`（可更新）；远端较新 → `failed`（需升级 Desktop）。
pub(crate) fn incompatible_failure(
    profile: &ConnectionProfile,
    evaluation: &CompatibilityEvaluation,
    remote_version: &str,
    local: &CompatibilityManifest,
    updated_at: u64,
) -> (ConnectionStateSnapshot, String) {
    let upgrade_server = evaluation.upgrade_target == Some(UpgradeTarget::Server);
    let mut state = ConnectionStateSnapshot::new(
        &profile.profile_id,
        profile.source_server_id.as_deref(),
        if upgrade_server {
            "needs-upgrade"
        } else {
            "failed"
        },
        updated_at,
    );
    state.current_version = Some(remote_version.to_owned());
    state.minimum_version = Some(evaluation.required_minimum_version.clone());
    state.upgrade_target = evaluation.upgrade_target.map(UpgradeTarget::as_str);
    state.error_code = Some("source-server-upgrade-required".to_owned());
    state.retryable = Some(false);
    if upgrade_server {
        state.update_availability = Some("required");
        state.available_version = Some(local.product_version.clone());
    }
    let message = if upgrade_server {
        format!(
            "remote source server {} is older than required minimum {}; update the source server",
            evaluation.older_version, evaluation.required_minimum_version
        )
    } else {
        format!(
            "remote source server {} requires controller {} or newer; update the desktop app",
            evaluation.newer_version, evaluation.required_minimum_version
        )
    };
    state.error = Some(message.clone());
    (state, message)
}
