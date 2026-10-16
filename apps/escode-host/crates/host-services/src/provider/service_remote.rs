//! Built-in Remote 刷新的调度（spec §12）：缓存启用且 Main 声明 `ESCODE_BUILTIN_REMOTE_REFRESH=1` 才建立。
//!
//! 只负责“何时刷新”：立即一次，之后每 60 s 一次（`Weak<Service>`，随服务消亡）。结果落在 Active 文件上，
//! 由 `load()` 的指纹轮询推进 Registry 修订号——这里不另开第二条写入 Registry 的路径。

use std::sync::Arc;
use std::time::Duration;

use escode_host_control::{LogLevel, logging};

use super::ProviderService;
use super::builtin_download::JsonGet;
use super::builtin_sync::{RefreshResult, Synchronizer};

const CHECK_INTERVAL: Duration = Duration::from_secs(60);

impl ProviderService {
    /// 启用 Remote 刷新（须已启用 Active 缓存，否则不生效）；`get` 是下载出口（生产 `HttpGet`，测试桩）。
    #[must_use]
    pub fn with_remote_refresh(mut self, get: Arc<dyn JsonGet>) -> Self {
        self.remote = Some(Arc::new(Synchronizer::new(Arc::clone(&self.builtin), get)));
        self
    }

    /// 启动周期刷新（幂等）。没有 `with_remote_refresh` 时什么也不做。
    pub fn start_remote_refresh(self: &Arc<Self>) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        if self
            .remote_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                if weak.upgrade().is_none() {
                    break;
                }
                report(remote.refresh(false).await);
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        });
    }
}

fn report(result: Result<RefreshResult, String>) {
    match result {
        Ok(RefreshResult::Updated) => logging::info("provider", "ESCode Built-in CDN 配置已更新"),
        Ok(other) => logging::log(
            "provider",
            LogLevel::Debug,
            vec![&format!("ESCode Built-in 刷新检查: {other:?}")],
        ),
        Err(error) => logging::warn(
            "provider",
            format!("ESCode Built-in Config 远端刷新失败: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_synchronizer_starting_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(ProviderService::new(
            dir.path().join("b.json"),
            dir.path().join("p.json"),
        ));
        service.start_remote_refresh();
    }
}
