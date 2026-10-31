//! 启动自动连接候选与 `lastOnlineAt` 串行落盘（spec §11.5）。
//!
//! Host 启动时只读取一次 settings 投影；本模块只投影候选并把写入交给单消费者队列，
//! 不持有连接状态。

use std::path::Path;

use tokio::sync::mpsc;

use super::*;

/// 自动恢复候选：每个 Source 只取一条活动 Profile（spec §11.5）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupReconnectCandidate {
    pub profile_id: String,
    pub source_server_id: String,
    pub last_online_at: u64,
}

/// settings 串行写队列的 `lastOnlineAt` 载荷。
#[derive(Clone, Debug)]
pub(crate) struct LastOnlineWrite {
    pub source_server_id: String,
    pub timestamp: u64,
}

impl SourceServerRegistry {
    /// 绑定 settings 根并启动 `lastOnlineAt` 单消费者写任务（spec §11.5）。
    ///
    /// 每个 Host 进程只启动一次；没有 tokio runtime 时保留内存更新，测试可直接调用
    /// [`Self::record_last_online_now`] 验证投影。
    pub fn start_last_online_writer(self: &std::sync::Arc<Self>) -> bool {
        let Some(home) = self.home.clone() else {
            return false;
        };
        if self.last_online_tx.get().is_some() {
            return false;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<LastOnlineWrite>();
        let _ = self.last_online_tx.set(tx);
        handle.spawn(async move {
            while let Some(write) = rx.recv().await {
                if let Err(error) = crate::settings::record_source_last_online(
                    &home,
                    &write.source_server_id,
                    write.timestamp,
                )
                .await
                {
                    escode_host_control::logging::warn(
                        "source-server",
                        format!(
                            "persist lastOnlineAt for {}: {error:#}",
                            write.source_server_id
                        ),
                    );
                }
            }
        });
        true
    }

    /// settings 根（仅 `load` 构造的 registry 有）。
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// 近 `window_ms` 内在线过、且仍有活动 Profile 的 Source（按最近在线倒序）。
    pub fn startup_candidates(&self, now: u64, window_ms: u64) -> Vec<StartupReconnectCandidate> {
        let state = self.lock();
        let mut candidates: Vec<StartupReconnectCandidate> = state
            .servers
            .iter()
            .filter_map(|server| {
                let last_online_at = server.last_online_at.filter(|value| *value > 0)?;
                if now.saturating_sub(last_online_at) > window_ms {
                    return None;
                }
                let profiles: Vec<&ConnectionProfile> = state
                    .profiles
                    .iter()
                    .filter(|profile| {
                        profile.lifecycle_state == "active"
                            && profile.source_server_id.as_deref()
                                == Some(server.source_server_id.as_str())
                    })
                    .collect();
                let preferred = server
                    .preferred_profile_id
                    .as_deref()
                    .and_then(|profile_id| {
                        profiles
                            .iter()
                            .find(|profile| profile.profile_id == profile_id)
                            .copied()
                    })
                    .or_else(|| {
                        profiles
                            .iter()
                            .max_by_key(|profile| profile.last_connected_at.unwrap_or(0))
                            .copied()
                    })?;
                Some(StartupReconnectCandidate {
                    profile_id: preferred.profile_id.clone(),
                    source_server_id: server.source_server_id.clone(),
                    last_online_at,
                })
            })
            .collect();
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.last_online_at));
        candidates
    }

    /// 同步推进内存投影，并把落盘写入交给串行队列；不存在的 Source 不重建。
    pub fn record_last_online_now(&self, source_server_id: &str, timestamp: u64) {
        {
            let mut registry = self.lock();
            let Some(server) = registry
                .servers
                .iter_mut()
                .find(|server| server.source_server_id == source_server_id)
            else {
                return;
            };
            if timestamp <= server.last_online_at.unwrap_or(0) {
                return;
            }
            server.last_online_at = Some(timestamp);
        }
        if let Some(tx) = self.last_online_tx.get() {
            let _ = tx.send(LastOnlineWrite {
                source_server_id: source_server_id.to_owned(),
                timestamp,
            });
        }
    }
}
