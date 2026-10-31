//! 首连取消（S5-a cancel，docs/specs/source-server-remote-rust-host.md §12.2）。
//!
//! 取消按 **caller key** 作用域：同一个 Profile 可能被多个窗口共享 single-flight，取消一个窗口不得
//! 中止其它窗口的在途连接。登记表只保存 caller → 取消信号，连接事实与尝试代次仍归
//! [`SourceServerConnections`]。

use std::sync::MutexGuard;

use anyhow::Result;
use tokio::sync::watch;

use super::SourceServerConnections;

/// 调用方主动取消首连：可重试，且不是远端/凭据故障。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionCanceled;

impl std::fmt::Display for ConnectionCanceled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("connection canceled by the caller")
    }
}

impl std::error::Error for ConnectionCanceled {}

impl SourceServerConnections {
    /// 登记一个可取消的在途连接等待者；调用方结束时必须 `finish_cancellable`。
    pub fn register_cancellable(&self, caller_key: &str) -> watch::Receiver<bool> {
        let (sender, receiver) = watch::channel(false);
        self.lock_cancellations()
            .insert(caller_key.to_owned(), sender);
        receiver
    }

    /// 注销 caller key（成功、失败、取消都走这里，避免取消表泄漏）。
    pub fn finish_cancellable(&self, caller_key: &str) {
        self.lock_cancellations().remove(caller_key);
    }

    /// 取消一个调用方的在途首连；未知/已完成的 key 返回 `false`（幂等、无副作用）。
    pub fn cancel_cancellable(&self, caller_key: &str) -> bool {
        let sender = self.lock_cancellations().get(caller_key).cloned();
        match sender {
            Some(sender) => {
                sender.send_replace(true);
                true
            }
            None => false,
        }
    }

    /// 可取消的 `connect`：取消时丢弃在建 future（transport `Child` 的 `kill_on_drop` 关闭 socket），
    /// 再递增尝试代次并收口状态，避免迟到的握手结果把 profile 拉回 online。
    pub async fn connect_cancellable(
        &self,
        profile_id: &str,
        cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        let connect = self.connect(profile_id);
        tokio::pin!(connect);
        tokio::select! {
            biased;
            () = wait_cancelled(cancel) => {
                escode_host_control::logging::info(
                    "source-server",
                    format!("canceled pending connect for profile {profile_id}"),
                );
                self.abort_pending_connect(profile_id).await;
                Err(anyhow::Error::new(ConnectionCanceled))
            }
            result = &mut connect => result,
        }
    }

    /// 取消收口：让旧的握手结果失效，并显式发布 `offline`（可重试、错误码 `remote-connection-canceled`）。
    async fn abort_pending_connect(&self, profile_id: &str) {
        let profile = self.registry().profile(profile_id);
        let previous = {
            let mut slots = self.lock();
            let slot = slots.entry(profile_id.to_owned()).or_default();
            slot.attempt += 1;
            slot.connection.take()
        };
        if let Some(connection) = previous {
            let _ = connection.shutdown().await;
        }
        if let Some(provisioning) = self.options().provisioning.as_ref() {
            provisioning.remove(profile_id);
        }
        let Some(profile) = profile else {
            return;
        };
        self.state().publish(
            profile.source_server_id.as_deref(),
            profile_id,
            "offline",
            Some(ConnectionCanceled.to_string()),
            Some("remote-connection-canceled"),
            Some(true),
            None,
        );
    }

    pub(crate) fn lock_cancellations(
        &self,
    ) -> MutexGuard<'_, std::collections::BTreeMap<String, watch::Sender<bool>>> {
        self.cancellations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 等待取消信号；发送端被注销（调用方已结束）后不再有取消可言，永久等待。
async fn wait_cancelled(mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow_and_update() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
#[path = "source_server_connections_cancel_tests.rs"]
mod tests;
