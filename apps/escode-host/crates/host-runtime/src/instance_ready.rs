//! ready/closed 状态通知：替代 `wait_ready` 的 20 ms 轮询（M2 报告 §9 技术债）。
//!
//! 状态只有三个：`Pending` → `Ready`（收到 `startup/storageState ready`）或 `Closed`
//! （读端关闭）。`Closed` 优先于 `Ready`：等待中的 `initializeWorkspace` 在实例崩溃后
//! 立即拿到 `false`，不必等超时。

use std::time::Duration;

use super::{Instance, InstanceState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadyState {
    Pending,
    Ready,
    Closed,
}

impl InstanceState {
    /// 帧泵收到 ready 帧：只在仍为 `Pending` 时转为 `Ready`（`Closed` 不可逆）。
    pub(super) fn mark_ready(&self) {
        self.ready.send_if_modified(|state| {
            if *state == ReadyState::Pending {
                *state = ReadyState::Ready;
                true
            } else {
                false
            }
        });
    }

    /// 读端关闭：唤醒所有等待方；晚到的 ready 帧不能再把状态改回 `Ready`。
    pub(super) fn mark_closed(&self) {
        self.ready.send_replace(ReadyState::Closed);
    }
}

impl Instance {
    /// 等运行时报到（`startup/storageState` phase=ready）；实例退出即提前返回 false。
    pub async fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut ready = self.state.ready.subscribe();
        loop {
            match *ready.borrow_and_update() {
                ReadyState::Ready => return true,
                ReadyState::Closed => return false,
                ReadyState::Pending => {}
            }
            match tokio::time::timeout_at(deadline, ready.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => return false,
            }
        }
    }
}
