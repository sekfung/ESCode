//! HttpModel 的计时辅助：首包 / 空闲截止时间（客户端与自定义 CA 见 `crates/net`）。

use std::time::Duration;
use tokio::time::Instant;

pub(super) fn after(ms: u64) -> Option<Instant> {
    if ms == 0 {
        None
    } else {
        Instant::now().checked_add(Duration::from_millis(ms))
    }
}

pub(super) async fn deadline(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
