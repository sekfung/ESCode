//! Source Server 自动恢复（S4-e，docs/specs/source-server-remote-rust-host.md §11.5）。
//!
//! 断线预算固定 2s → 4s → 8s ×3；预算耗尽发布 `offline` +
//! `remote-recovery-budget-exhausted`。启动自动连接只对近 7 天在线过的 Source 各尝试一次，
//! ≤3 并发、单次 10s、静默（不安装、不升级、不弹凭据）。

use std::sync::Arc;
use std::time::Duration;

use crate::source_server::{ConnectionStateSnapshot, StartupReconnectCandidate};
use crate::source_server_connection_state::{
    RECOVERY_BUDGET_EXHAUSTED_CODE, RECOVERY_LIMIT, now_ms, recovery_exhausted_state,
    retrying_state,
};
use crate::source_server_connections::{ConnectIntent, SourceServerConnections};

/// 启动自动连接的 7 天门禁。
pub const STARTUP_RECONNECT_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// 启动自动连接单次上限与并发（首个候选独占启动窗口）。
pub const STARTUP_RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const STARTUP_RECONNECT_CONCURRENCY: usize = 3;

impl SourceServerConnections {
    /// 断线后的自动恢复循环；`generation` 是断线时 online 连接的尝试代次。
    ///
    /// watch_close 已发布 attempt=1；后续每档延迟前先发布 `retrying`，避免 4s/8s 等待期间
    /// UI 停在 `failed`。手动连接/断开/删除会推进代次，过期恢复静默退出。
    pub(crate) async fn run_recovery(self: Arc<Self>, profile_id: String, generation: u64) {
        let delays = self.options().recovery_delays;
        let mut generation = generation;
        let mut last_error = Some("remote transport closed".to_owned());
        for (index, delay) in delays.into_iter().enumerate() {
            if index > 0 {
                if self.current_attempt(&profile_id) != generation {
                    return;
                }
                let Some(previous) = self.registry().connection_state(&profile_id) else {
                    return;
                };
                if is_terminal_recovery_state(&previous) {
                    return;
                }
                self.registry().set_connection_state(retrying_state(
                    &previous,
                    last_error.clone(),
                    (index + 1) as u32,
                    RECOVERY_LIMIT,
                ));
            }
            tokio::time::sleep(delay).await;
            if self.current_attempt(&profile_id) != generation {
                return;
            }
            // connect_with 在取得单飞锁后推进代次；失败后不能用“当前代次”回填，
            // 否则恰好在此期间发生的手动断开会被误认成新一代并继续恢复。
            let expected_attempt = self.current_attempt(&profile_id).saturating_add(1);
            match self.connect_with(&profile_id, ConnectIntent::Recover).await {
                Ok(()) => return,
                Err(error) => {
                    let state = self.registry().connection_state(&profile_id);
                    if state.as_ref().is_some_and(is_terminal_recovery_state) {
                        return;
                    }
                    if self.current_attempt(&profile_id) != expected_attempt {
                        return;
                    }
                    last_error = state
                        .as_ref()
                        .and_then(|state| state.error.clone())
                        .or_else(|| Some(format!("{error:#}")));
                    generation = expected_attempt;
                }
            }
        }
        let previous = self
            .registry()
            .connection_state(&profile_id)
            .unwrap_or_else(|| {
                ConnectionStateSnapshot::new(&profile_id, None, "offline", now_ms())
            });
        self.registry()
            .set_connection_state(recovery_exhausted_state(
                &previous,
                last_error,
                RECOVERY_LIMIT,
            ));
        escode_host_control::logging::warn(
            "source-server",
            format!(
                "recovery budget exhausted profile={profile_id} code={RECOVERY_BUDGET_EXHAUSTED_CODE}"
            ),
        );
    }

    /// Host 启动自动连接：≤3 并发；每个 Source 只试一次，单次 10s。
    pub async fn startup_reconnect(self: &Arc<Self>, candidates: Vec<StartupReconnectCandidate>) {
        let gate = Arc::new(tokio::sync::Semaphore::new(STARTUP_RECONNECT_CONCURRENCY));
        let mut tasks = Vec::new();
        for candidate in candidates {
            let manager = Arc::clone(self);
            let gate = Arc::clone(&gate);
            tasks.push(tokio::spawn(async move {
                let Ok(_permit) = gate.acquire_owned().await else {
                    return;
                };
                manager.startup_attempt(candidate).await;
            }));
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    async fn startup_attempt(self: &Arc<Self>, candidate: StartupReconnectCandidate) {
        let profile_id = candidate.profile_id.clone();
        // connect_with 在取得单飞锁后推进代次；超时只在“仍是本次启动尝试”时写状态，
        // 避免覆盖随后发生的用户手动连接（spec §11.5 迟到写入不覆盖新事实）。
        let attempt = self.current_attempt(&profile_id).saturating_add(1);
        let result = tokio::time::timeout(
            self.options().startup_timeout,
            self.connect_with(&profile_id, ConnectIntent::Recover),
        )
        .await;
        match result {
            Ok(Ok(())) => escode_host_control::logging::info(
                "source-server",
                format!("startup reconnect online profile={profile_id}"),
            ),
            Ok(Err(error)) => escode_host_control::logging::warn(
                "source-server",
                format!("startup reconnect failed profile={profile_id}: {error:#}"),
            ),
            Err(_) => {
                escode_host_control::logging::warn(
                    "source-server",
                    format!(
                        "startup reconnect timed out profile={profile_id} after {:?}",
                        self.options().startup_timeout
                    ),
                );
                if self.current_attempt(&profile_id) == attempt
                    && self
                        .registry()
                        .connection_state(&profile_id)
                        .is_some_and(|state| state.state == "bootstrapping")
                {
                    self.publish_startup_timeout(&profile_id, &candidate.source_server_id);
                }
            }
        }
    }

    fn publish_startup_timeout(&self, profile_id: &str, source_server_id: &str) {
        let mut state = self
            .registry()
            .connection_state(profile_id)
            .unwrap_or_else(|| {
                ConnectionStateSnapshot::new(profile_id, Some(source_server_id), "failed", now_ms())
            });
        state.state = "failed";
        state.error = Some(format!(
            "startup reconnect timed out after {:?}",
            self.options().startup_timeout
        ));
        state.error_code = Some("remote-connection-timeout".to_owned());
        state.retryable = Some(true);
        state.updated_at = now_ms();
        self.registry().set_connection_state(state);
    }
}

/// 自动恢复遇到用户动作/安装/升级终态时停止，不再覆盖用户可见状态。
fn is_terminal_recovery_state(state: &ConnectionStateSnapshot) -> bool {
    matches!(
        state.state,
        "needs-install" | "needs-upgrade" | "identity-change-pending" | "online" | "updating"
    ) || (state.state == "failed" && state.retryable == Some(false))
}
