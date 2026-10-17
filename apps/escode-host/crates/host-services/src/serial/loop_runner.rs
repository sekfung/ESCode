//! 定时循环发送（S4）：TS `serialLoop.ts` 的 Rust 对应物。
//!
//! 会话持有 `Arc<LoopState>`：状态供 `onStatus` 渲染，`stop` 供关闭/断开时协作停止。
//! 任务本身只依赖注入的写函数与回调，因此可以脱离会话单测。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;

/// TS `SERIAL_LOOP_MIN_INTERVAL_MS`。
pub const MIN_INTERVAL_MS: u64 = 10;
/// TS `PROGRESS_THROTTLE_MS`：启动、停止立即推送，进度事件不低于这个间隔。
pub const PROGRESS_THROTTLE: Duration = Duration::from_millis(250);

pub type WriteFuture = Pin<Box<dyn Future<Output = bool> + Send>>;
pub type WriteFn = Arc<dyn Fn(Vec<u8>) -> WriteFuture + Send + Sync>;
pub type Callback = Arc<dyn Fn(u64) + Send + Sync>;

/// 参数校验（TS `startLoop` 的 `invalid` 表达式）。
pub fn is_valid_params(interval_ms: u64, byte_len: usize, count: Option<u64>) -> bool {
    interval_ms >= MIN_INTERVAL_MS && byte_len > 0 && count.is_none_or(|count| count >= 1)
}

/// 一次循环任务的状态：进度由任务写、状态事件由会话读。
pub struct LoopState {
    pub generation: u64,
    pub interval_ms: u64,
    pub count: Option<u64>,
    sent: AtomicU64,
    stop: AtomicBool,
    wake: Notify,
}

impl LoopState {
    pub fn new(generation: u64, interval_ms: u64, count: Option<u64>) -> Arc<Self> {
        Arc::new(Self {
            generation,
            interval_ms,
            count,
            sent: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            wake: Notify::new(),
        })
    }

    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::SeqCst)
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// 协作停止：任务在下一个 tick 前退出（进行中的写入会跑完，TS 同）。
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn to_json(&self) -> Value {
        let mut value = Map::new();
        value.insert("intervalMs".to_owned(), Value::from(self.interval_ms));
        if let Some(count) = self.count {
            value.insert("count".to_owned(), Value::from(count));
        }
        value.insert("sent".to_owned(), Value::from(self.sent()));
        Value::Object(value)
    }
}

/// 启动任务：立即发送第一次（`interval` 的首个 tick），之后按固定间隔；任务不排队积压。
pub fn spawn(
    state: Arc<LoopState>,
    bytes: Vec<u8>,
    write: WriteFn,
    on_progress: Callback,
    on_finished: Callback,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(state.interval_ms));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_progress = Instant::now();
        loop {
            tokio::select! {
                // 先被 stop 唤醒就不再等下一个 tick（间隔可能很长）。
                _ = state.wake.notified() => {}
                _ = ticker.tick() => {}
            }
            if state.is_stopped() {
                return;
            }
            if !write(bytes.clone()).await {
                on_finished(state.generation);
                return;
            }
            let sent = state.sent.fetch_add(1, Ordering::SeqCst) + 1;
            // TS 在写入后才检查 stopped：已停的循环不再发进度/结束事件。
            if state.is_stopped() {
                return;
            }
            if state.count.is_some_and(|count| sent >= count) {
                on_finished(state.generation);
                return;
            }
            if last_progress.elapsed() >= PROGRESS_THROTTLE {
                last_progress = Instant::now();
                on_progress(state.generation);
            }
        }
    });
}

#[cfg(test)]
#[path = "loop_runner_tests.rs"]
mod tests;
