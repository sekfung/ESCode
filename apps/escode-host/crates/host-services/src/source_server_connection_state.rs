//! Source Server 连接状态写入（S3-a/S4-c）：`failed` / `needs-install` / `bootstrapping`。
//!
//! 状态唯一写到 registry；本模块只做投影与 operation log，不持有连接或尝试代次。

use crate::source_server::{
    ConnectionOperationLog, ConnectionProfile, ConnectionStateSnapshot, IdentityChangePending,
    SourceServerRegistry,
};
use crate::source_server_compatibility::{CompatibilityEvaluation, CompatibilityManifest};
use crate::source_server_connection_projection::incompatible_failure;
use crate::source_server_release::InstallPreview;

pub(crate) struct StateWriter<'a> {
    pub registry: &'a SourceServerRegistry,
    pub compatibility: &'a CompatibilityManifest,
}

impl StateWriter<'_> {
    /// 写 `failed` 并返回原错误（调用方直接上抛，不吞错）。
    pub fn fail(
        &self,
        profile: &ConnectionProfile,
        error: anyhow::Error,
        error_code: Option<&str>,
    ) -> anyhow::Error {
        // 主机 key 变化是安全事件：即使由 preflight/安装阶段上报，也不能标记为可自动重试。
        let retryable = Some(error_code != Some("remote-host-key-changed"));
        self.publish(
            profile.source_server_id.as_deref(),
            &profile.profile_id,
            "failed",
            Some(format!("{error:#}")),
            error_code,
            retryable,
            None,
        );
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                &profile.profile_id,
                "connect",
                "failed",
                "error",
                format!("{error:#}"),
                now_ms(),
            ));
        error
    }

    /// 主机 key 已变化：安全事件，显式失败且不可通过自动重试消除。
    pub fn fail_host_key_changed(
        &self,
        profile: &ConnectionProfile,
        error: anyhow::Error,
    ) -> anyhow::Error {
        let message = format!("{error:#}");
        self.publish(
            profile.source_server_id.as_deref(),
            &profile.profile_id,
            "failed",
            Some(message.clone()),
            Some("remote-host-key-changed"),
            Some(false),
            None,
        );
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                &profile.profile_id,
                "connect",
                "failed",
                "error",
                message,
                now_ms(),
            ));
        error
    }

    /// 安装准备完成：先发布带 preview 的 `needs-install`（UI 展示确认信息），
    /// 再回到 `bootstrapping` 表示正在安装（spec §11.3；安装期间不发布 online）。
    pub fn publish_install(&self, profile: &ConnectionProfile, preview: &Option<InstallPreview>) {
        for state in ["needs-install", "bootstrapping"] {
            let mut snapshot = ConnectionStateSnapshot::new(
                &profile.profile_id,
                profile.source_server_id.as_deref(),
                state,
                now_ms(),
            );
            snapshot.install_preview.clone_from(preview);
            self.registry.set_connection_state(snapshot);
        }
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                &profile.profile_id,
                "connect",
                "installing",
                "info",
                "[source-server:install] installing missing components".to_owned(),
                now_ms(),
            ));
    }

    /// 远端持久身份变化：零写入，发布待确认状态；用户确认后才会重绑定 Profile。
    pub fn identity_change_pending(
        &self,
        profile: &ConnectionProfile,
        pending_source_server_id: &str,
    ) -> anyhow::Error {
        let error = IdentityChangePending {
            profile_id: profile.profile_id.clone(),
            pending_source_server_id: pending_source_server_id.to_owned(),
        };
        let mut state = ConnectionStateSnapshot::new(
            &profile.profile_id,
            profile.source_server_id.as_deref(),
            "identity-change-pending",
            now_ms(),
        );
        state.pending_source_server_id = Some(pending_source_server_id.to_owned());
        state.error_code = Some("source-server-identity-change-pending".to_owned());
        state.retryable = Some(false);
        state.error = Some(error.to_string());
        self.registry.set_connection_state(state);
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                &profile.profile_id,
                "connect",
                "identity-change-pending",
                "warn",
                error.to_string(),
                now_ms(),
            ));
        anyhow::Error::new(error)
    }

    /// 兼容失败：远端较旧 → `needs-upgrade`（可更新）；远端较新 → `failed`（需升级 Desktop）。
    pub fn fail_incompatible(
        &self,
        profile: &ConnectionProfile,
        evaluation: &CompatibilityEvaluation,
        remote_version: &str,
    ) -> anyhow::Error {
        let (state, message) = incompatible_failure(
            profile,
            evaluation,
            remote_version,
            self.compatibility,
            now_ms(),
        );
        self.registry.set_connection_state(state);
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                &profile.profile_id,
                "connect",
                "failed",
                "error",
                message.clone(),
                now_ms(),
            ));
        anyhow::anyhow!(message)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn publish(
        &self,
        source_server_id: Option<&str>,
        profile_id: &str,
        state: &'static str,
        error: Option<String>,
        error_code: Option<&str>,
        retryable: Option<bool>,
        current_version: Option<&str>,
    ) {
        let mut snapshot =
            ConnectionStateSnapshot::new(profile_id, source_server_id, state, now_ms());
        snapshot.error = error;
        snapshot.error_code = error_code.map(str::to_owned);
        snapshot.retryable = retryable;
        snapshot.current_version = current_version.map(str::to_owned);
        self.registry.set_connection_state(snapshot);
    }
}

pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// 自动恢复预算耗尽错误码（spec §11.5）。
pub const RECOVERY_BUDGET_EXHAUSTED_CODE: &str = "remote-recovery-budget-exhausted";
/// 自动恢复最多尝试次数（2s → 4s → 8s）。
pub const RECOVERY_LIMIT: u32 = 3;

/// online 后链路断开的中间态：保留最后一次握手兼容事实，只附加恢复计数。
pub fn retrying_state(
    previous: &ConnectionStateSnapshot,
    error: Option<String>,
    attempt: u32,
    limit: u32,
) -> ConnectionStateSnapshot {
    let mut state = previous.clone();
    state.state = "retrying";
    state.error = error;
    state.error_code = None;
    state.retryable = Some(true);
    state.recovery_attempt = Some(attempt);
    state.recovery_limit = Some(limit);
    state.recovery_budget_exhausted = None;
    state.updated_at = now_ms();
    state
}

/// 恢复预算耗尽：终态 `offline`，显式暴露预算耗尽码与计数。
pub fn recovery_exhausted_state(
    previous: &ConnectionStateSnapshot,
    error: Option<String>,
    limit: u32,
) -> ConnectionStateSnapshot {
    let mut state = previous.clone();
    state.state = "offline";
    state.error = Some(error.unwrap_or_else(|| "remote recovery budget exhausted".to_owned()));
    state.error_code = Some(RECOVERY_BUDGET_EXHAUSTED_CODE.to_owned());
    state.retryable = Some(true);
    state.recovery_attempt = Some(limit);
    state.recovery_limit = Some(limit);
    state.recovery_budget_exhausted = Some(true);
    state.updated_at = now_ms();
    state
}
