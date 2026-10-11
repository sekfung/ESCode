//! stdin 读取与输出停滞看门狗（docs/specs/rust-runtime-performance.md「输出背压下的 EOF/退出」）。
//!
//! 修复：读取线程原先直接 `blocking_send` 到容量 64 的输入队列；Host 停止读取 stdout 时，
//! 输出通道写满 → engine 阻塞在发送 → 输入队列不再被消费 → 读取线程阻塞在入队上，
//! 永远读不到 EOF，进程无法退出。现在读取线程只写入按字节限额的中间队列，阻塞由转发线程承担，
//! EOF 总能被及时观测；EOF 之后再按「写出是否仍有进展」区分仍在读取的 Host 与已离开的 Host。
use crate::domain::MAX_REQUEST_BYTES;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::contract::Input;

/// 读取线程只在待派发字节超过该值时才停下（Host 是受信任的父进程，Node 侧甚至不设上限）。
const MAX_PENDING_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// EOF 之后单次写出卡住这么久，视为 Host 已不再读取 stdout。
const STALLED_WRITE_AFTER_EOF_MS: u64 = 1_000;

enum Line {
    Data(Vec<u8>),
    TooLarge,
}

type Pending = Arc<(Mutex<usize>, Condvar)>;

/// 启动读取线程与转发线程；读到 EOF（或超长行）时立即取消 `input_closed`。
pub(crate) fn spawn_reader(
    in_tx: mpsc::Sender<Input>,
    input_closed: CancellationToken,
    dispatch: fn(&mpsc::Sender<Input>, &[u8]) -> bool,
) {
    let pending: Pending = Arc::new((Mutex::new(0), Condvar::new()));
    let (line_tx, line_rx) = std::sync::mpsc::channel::<Line>();
    let forward_pending = pending.clone();
    std::thread::spawn(move || {
        while let Ok(line) = line_rx.recv() {
            let (size, keep_going) = match line {
                Line::Data(bytes) => (bytes.len(), dispatch(&in_tx, &bytes)),
                Line::TooLarge => {
                    let _ = in_tx.blocking_send(Input::TooLarge);
                    return;
                }
            };
            let (lock, ready) = &*forward_pending;
            *lock.lock().unwrap() -= size;
            ready.notify_all();
            if !keep_going {
                return;
            }
        }
        let _ = in_tx.blocking_send(Input::Eof);
    });
    std::thread::spawn(move || {
        let push = |line: Line| {
            if let Line::Data(bytes) = &line {
                let (lock, ready) = &*pending;
                let mut used = lock.lock().unwrap();
                while *used > MAX_PENDING_INPUT_BYTES {
                    used = ready.wait(used).unwrap();
                }
                *used += bytes.len();
            }
            line_tx.send(line).is_ok()
        };
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        let mut line = Vec::new();
        while let Ok(available) = reader.fill_buf() {
            if available.is_empty() {
                if !line.is_empty() {
                    push(Line::Data(std::mem::take(&mut line)));
                }
                break;
            }
            let take = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |i| i + 1);
            if line.len() + take > MAX_REQUEST_BYTES {
                push(Line::TooLarge);
                break;
            }
            line.extend_from_slice(&available[..take]);
            reader.consume(take);
            if line.last() == Some(&b'\n') && !push(Line::Data(std::mem::take(&mut line))) {
                return;
            }
        }
        input_closed.cancel();
    });
}

/// 写出线程的进度：当前写入开始的毫秒时间（0 = 空闲）与是否已结束。
#[derive(Clone, Default)]
pub(crate) struct WriteProgress {
    started_ms: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
}

impl WriteProgress {
    pub(crate) fn begin(&self) {
        self.started_ms.store(now_ms().max(1), Ordering::SeqCst);
    }
    pub(crate) fn end(&self) {
        self.started_ms.store(0, Ordering::SeqCst);
    }
    pub(crate) fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }
    fn stalled(&self, now: u64) -> bool {
        let started = self.started_ms.load(Ordering::SeqCst);
        started != 0 && now.saturating_sub(started) >= STALLED_WRITE_AFTER_EOF_MS
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// EOF 之后：写出仍有进展就照常排空（在途响应不丢）；单次写出卡住超过阈值 → 取消运行时，
/// 与 POSIX 的 SIGTERM 走同一条收尾路径。
pub(crate) fn watch_stalled_output(
    progress: WriteProgress,
    input_closed: CancellationToken,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = input_closed.cancelled() => {}
        }
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tick.tick() => {}
            }
            if progress.finished.load(Ordering::SeqCst) {
                return;
            }
            if progress.stalled(now_ms()) {
                cancel.cancel();
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_long_running_write_counts_as_stalled() {
        let progress = WriteProgress::default();
        assert!(!progress.stalled(now_ms()), "idle writer is not stalled");
        progress.begin();
        let started = progress.started_ms.load(Ordering::SeqCst);
        assert!(!progress.stalled(started + STALLED_WRITE_AFTER_EOF_MS - 1));
        assert!(progress.stalled(started + STALLED_WRITE_AFTER_EOF_MS));
        progress.end();
        assert!(!progress.stalled(started + 10 * STALLED_WRITE_AFTER_EOF_MS));
    }
}
