//! `onDidChange` 的变更源（docs/specs/rust-u2-providers.md §9）。
//!
//! 唯一所有者：`ProviderService::load()` 在 Registry 内容真正变化（修订号推进）时广播新修订号；
//! 本进程自己的写入与外部改文件走同一条路径。TS 里 Personal 靠 1s 轮询、Built-in 靠文件监听，Rust 统一用
//! 1s 的指纹轮询（指纹未变不读盘，见 `load`），首个订阅者出现时才启动轮询。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::watch;

use super::ProviderService;

pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(1);

impl ProviderService {
    /// 测试用：缩短轮询间隔。
    #[cfg(test)]
    pub(super) fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// 订阅 Registry 变更。先加载一次（初始快照不算变化），再登记接收端：之后每个新修订号唤醒一次。
    pub async fn watch_changes(self: &Arc<Self>) -> watch::Receiver<u64> {
        // 加载失败（如 Built-in 暂不可用）不阻止订阅：恢复后的首次成功加载会推进修订号并通知。
        let _ = self.load().await;
        self.ensure_polling();
        self.changes.subscribe()
    }

    fn ensure_polling(self: &Arc<Self>) {
        if self.polling.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let interval = self.poll_interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(service) = weak.upgrade() else {
                    break;
                };
                // 失败只会让下一轮重试；调用方（订阅者）关心的是“有新的可用快照”。
                let _ = service.load().await;
            }
        });
    }
}

#[cfg(test)]
#[path = "service_events_tests.rs"]
mod tests;
