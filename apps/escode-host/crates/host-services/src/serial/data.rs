//! 会话的数据面（S3）：RX 合并（尾部静默窗口）+ TX 记账（`rust-u2-serial.md` §10）。
//!
//! `ChunkBuffer` 是纯数据结构（§10）；本模块负责**定时语义**与 seq 分配：
//! - RX：字节先进 pending，窗口内没有新字节才成段（TS `coalesceWindowMs` = 10ms 的尾部去抖），
//!   或 pending 超过 `coalesceMaxBytes`（4096）时立即成段；
//! - TX：先 flush pending RX 再记 TX（保证回显出现在设备响应之前），seq 在记录时分配。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::buffer::{Chunk, ChunkBuffer};

/// TS `DEFAULT_BUFFER_LIMIT_BYTES`。
pub const BUFFER_LIMIT_BYTES: usize = 1024 * 1024;
/// TS `DEFAULT_COALESCE_WINDOW_MS`。
pub const COALESCE_WINDOW: Duration = Duration::from_millis(10);
/// TS `DEFAULT_COALESCE_MAX_BYTES`。
pub const COALESCE_MAX_BYTES: usize = 4096;
/// TS `SERIAL_WRITE_LIMIT_BYTES`。
pub const WRITE_LIMIT_BYTES: usize = 64 * 1024;

struct PendingRx {
    at: u64,
    parts: Vec<Vec<u8>>,
    len: usize,
}

/// 一个会话的收发记录：缓冲 + seq 分配 + 去抖定时。
pub struct DataSink {
    buffer: Mutex<ChunkBuffer>,
    seq: Arc<AtomicU64>,
    on_chunk: Arc<dyn Fn(Chunk) + Send + Sync>,
    pending: Mutex<Option<PendingRx>>,
    notify: Arc<Notify>,
    flush_task: Mutex<Option<JoinHandle<()>>>,
}

impl DataSink {
    pub fn new(seq: Arc<AtomicU64>, on_chunk: Arc<dyn Fn(Chunk) + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            buffer: Mutex::new(ChunkBuffer::new(BUFFER_LIMIT_BYTES)),
            seq,
            on_chunk,
            pending: Mutex::new(None),
            notify: Arc::new(Notify::new()),
            flush_task: Mutex::new(None),
        })
    }

    pub fn last_seq(&self) -> u64 {
        self.lock(&self.buffer).last_seq()
    }

    pub fn snapshot(&self) -> Value {
        self.lock(&self.buffer).snapshot()
    }

    pub fn clear(&self) {
        self.lock(&self.buffer).clear();
    }

    /// 字节上送：进 pending；超过合并上限立即成段，否则等尾部静默窗口。
    pub fn receive(self: &Arc<Self>, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let overflow = {
            let mut pending = self.lock(&self.pending);
            let entry = pending.get_or_insert_with(|| PendingRx {
                at: now_ms(),
                parts: Vec::new(),
                len: 0,
            });
            entry.parts.push(bytes);
            entry.len += entry.parts.last().map(Vec::len).unwrap_or(0);
            entry.len > COALESCE_MAX_BYTES
        };
        if overflow {
            self.flush_rx();
            return;
        }
        self.notify.notify_one();
        self.ensure_flusher();
    }

    /// 尾部静默窗口：一个任务等到「窗口内没有新字节」才 flush。
    fn ensure_flusher(self: &Arc<Self>) {
        let mut slot = self.lock(&self.flush_task);
        if slot.is_some() {
            return;
        }
        let sink = Arc::clone(self);
        *slot = Some(tokio::spawn(async move {
            loop {
                sink.notify.notified().await;
                // 每来一批字节就重新计时；静默满一个窗口即收口。
                while tokio::time::timeout(COALESCE_WINDOW, sink.notify.notified())
                    .await
                    .is_ok()
                {}
                sink.flush_rx();
            }
        }));
    }

    /// 收口 pending RX（`clear`/`close`/`write` 前也会调用）。
    pub fn flush_rx(&self) {
        let pending = self.lock(&self.pending).take();
        let Some(pending) = pending else {
            return;
        };
        if pending.len == 0 {
            return;
        }
        let bytes: Vec<u8> = pending.parts.concat();
        let seq = Arc::clone(&self.seq);
        let mut allocate = || seq.fetch_add(1, Ordering::SeqCst) + 1;
        let chunk = self
            .lock(&self.buffer)
            .push_rx(bytes, pending.at, &mut allocate);
        (self.on_chunk)(chunk);
    }

    /// 记一次 TX（`write` 在真正写出之前调用，保证 seq 先于设备回显）。
    pub fn push_tx(&self, source: &str, bytes: Vec<u8>, session_id: Option<String>) -> u64 {
        self.flush_rx();
        let seq = Arc::clone(&self.seq);
        let mut allocate = || seq.fetch_add(1, Ordering::SeqCst) + 1;
        let chunk =
            self.lock(&self.buffer)
                .push_tx(source, bytes, now_ms(), session_id, &mut allocate);
        let seq = chunk.seq;
        (self.on_chunk)(chunk);
        seq
    }

    /// 停止定时任务（关闭会话时调用），并把未成段的 RX 收口。
    pub fn shutdown(&self) {
        if let Some(task) = self.lock(&self.flush_task).take() {
            task.abort();
        }
        self.flush_rx();
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
