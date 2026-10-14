//! 有界队列（rust-u2-host-skeleton.md §3.2）：每个队列在构造处写明名称、容量与
//! **溢出策略**，并提供深度/丢弃计数指标。策略不是"满了再说"，而是通道的一部分：
//! 调用方只能通过 [`channel`] 建队，策略随队列进入类型系统。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

/// 溢出策略（与子 spec §3.2 的表一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overflow {
    /// 背压：生产者等待（Renderer→Host 入站；读侧阻塞即协议背压）。
    Backpressure,
    /// 丢弃可重建流并计数（Host→Renderer 出站：快照恢复能补齐，控制帧不丢由 lane 保序负责）。
    DropReconstructible,
    /// 阻塞生产者（runtime→Host 事件：满时阻塞 runtime 侧，承接既有语义）。
    BlockProducer,
    /// 丢弃最低级别并计数（日志批量）。
    DropLowestPriority,
}

/// 队列指标：深度由通道自身反映，这里记录溢出与丢弃次数。
#[derive(Debug, Default)]
pub struct Metrics {
    pub dropped: AtomicU64,
    pub backpressured: AtomicU64,
}

impl Metrics {
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn backpressured(&self) -> u64 {
        self.backpressured.load(Ordering::Relaxed)
    }
}

/// 一条命名队列的发送端：策略在这里生效，指标在 [`Metrics`]。
#[derive(Clone)]
pub struct Sender<T> {
    name: &'static str,
    policy: Overflow,
    tx: mpsc::Sender<T>,
    metrics: Arc<Metrics>,
}

/// 队列名（日志与测试断言用）。
pub fn channel<T>(
    name: &'static str,
    capacity: usize,
    policy: Overflow,
) -> (Sender<T>, mpsc::Receiver<T>) {
    let (tx, rx) = mpsc::channel(capacity);
    let sender = Sender {
        name,
        policy,
        tx,
        metrics: Arc::new(Metrics::default()),
    };
    (sender, rx)
}

impl<T> Sender<T> {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn policy(&self) -> Overflow {
        self.policy
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        Arc::clone(&self.metrics)
    }

    pub fn capacity(&self) -> usize {
        self.tx.capacity()
    }

    /// 发送，按策略处理队列已满：
    ///
    /// - `Backpressure` / `BlockProducer`：等待（`await` 即阻塞生产者）；
    /// - `DropReconstructible` / `DropLowestPriority`：丢弃并计数。
    ///
    /// 返回是否已投递（`false` = 按策略丢弃，或接收端已关闭）。
    pub async fn send(&self, item: T) -> bool {
        match self.policy {
            Overflow::Backpressure | Overflow::BlockProducer => {
                // 先试投递：队满才等待并计一次背压（生产者阻塞即协议背压）。
                match self.tx.try_send(item) {
                    Ok(()) => true,
                    Err(mpsc::error::TrySendError::Closed(_)) => false,
                    Err(mpsc::error::TrySendError::Full(item)) => {
                        self.metrics.backpressured.fetch_add(1, Ordering::Relaxed);
                        match self.tx.reserve().await {
                            Ok(permit) => {
                                permit.send(item);
                                true
                            }
                            Err(_) => false,
                        }
                    }
                }
            }
            Overflow::DropReconstructible | Overflow::DropLowestPriority => {
                match self.tx.try_send(item) {
                    Ok(()) => true,
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
                        false
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => false,
                }
            }
        }
    }

    /// 记录一次背压等待（调用方在 send 前自行判断时使用，保证指标语义一致）。
    pub fn note_backpressure(&self) {
        self.metrics.backpressured.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backpressure_waits_until_the_consumer_drains() {
        let (tx, mut rx) = channel::<u32>("test-inbound", 1, Overflow::Backpressure);
        assert!(tx.send(1).await, "首个元素立即入队");
        let blocked = tokio::spawn({
            let tx = tx.clone();
            async move { tx.send(2).await }
        });
        tokio::task::yield_now().await;
        assert!(!blocked.is_finished(), "队列满时生产者等待（背压）");
        assert_eq!(rx.recv().await, Some(1));
        assert!(blocked.await.expect("join"), "消费者腾空后送达");
        assert_eq!(rx.recv().await, Some(2));
    }

    #[tokio::test]
    async fn reconstructible_drops_and_counts_instead_of_blocking() {
        let (tx, _rx) = channel::<u32>("test-outbound", 1, Overflow::DropReconstructible);
        assert!(tx.send(1).await);
        assert!(!tx.send(2).await, "队列满时丢弃可重建流");
        assert_eq!(tx.metrics().dropped(), 1);
        assert_eq!(tx.name(), "test-outbound");
        assert_eq!(tx.policy(), Overflow::DropReconstructible);
    }

    #[tokio::test]
    async fn closed_receiver_reports_not_delivered_for_both_policies() {
        let (tx, rx) = channel::<u32>("test-closed", 1, Overflow::DropLowestPriority);
        drop(rx);
        assert!(!tx.send(1).await);
        let (tx, rx) = channel::<u32>("test-closed-blocking", 1, Overflow::BlockProducer);
        drop(rx);
        assert!(!tx.send(1).await);
    }
}
