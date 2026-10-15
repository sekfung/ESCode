//! pty 输出 → 监听者的异步投递（TS `Emitter.fire` 的等价物）。
//!
//! 读线程（有界通道）→ [`Pump`] → 每个监听者各一条有界通道。没有监听者时数据直接丢弃（TS：无人订阅的
//! `Emitter` 不缓冲）；有监听者但消费慢时，这里的 `send` 等待，从而把压力沿通道一路传回 pty。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};

/// 子进程退出后继续排空读端的空闲上限：后台进程仍持有 pty 时读端不会 EOF，不能无限等。
const DRAIN_IDLE: Duration = Duration::from_millis(250);
const DATA_CAPACITY: usize = 64;
const EXIT_CAPACITY: usize = 4;

#[derive(Default)]
pub struct Listeners {
    data: Mutex<Vec<mpsc::Sender<String>>>,
    exit: Mutex<Vec<mpsc::Sender<i64>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Listeners {
    pub fn add_data(&self) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(DATA_CAPACITY);
        lock(&self.data).push(tx);
        rx
    }

    pub fn add_exit(&self) -> mpsc::Receiver<i64> {
        let (tx, rx) = mpsc::channel(EXIT_CAPACITY);
        lock(&self.exit).push(tx);
        rx
    }

    /// 摘除全部监听者（接收端随之结束）。
    pub fn clear(&self) {
        lock(&self.data).clear();
        lock(&self.exit).clear();
    }
}

/// 向一组监听者投递一个值；`cancel` 触发（dispose）时放弃并返回 `false`。
async fn deliver<T: Clone>(
    senders: &Mutex<Vec<mpsc::Sender<T>>>,
    value: T,
    cancel: &mut watch::Receiver<bool>,
) -> bool {
    let snapshot: Vec<mpsc::Sender<T>> = lock(senders).clone();
    for sender in &snapshot {
        tokio::select! {
            biased;
            _ = cancel.changed() => return false,
            _ = sender.send(value.clone()) => {}
        }
    }
    lock(senders).retain(|sender| !sender.is_closed());
    true
}

pub struct Pump {
    pub id: String,
    pub terminal_listeners: Arc<Listeners>,
    pub cancel: watch::Receiver<bool>,
    pub chunks: mpsc::Receiver<String>,
    pub exit: oneshot::Receiver<i64>,
    /// 自然退出收尾（从终端表摘除）。
    pub on_exit: Box<dyn FnOnce(&str) + Send>,
}

impl Pump {
    pub fn spawn(self) {
        tokio::spawn(self.run());
    }

    async fn run(mut self) {
        let listeners = Arc::clone(&self.terminal_listeners);
        let mut exit_code: Option<i64> = None;
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.changed() => return,
                chunk = self.chunks.recv() => match chunk {
                    Some(text) => {
                        if !deliver(&listeners.data, text, &mut self.cancel).await {
                            return;
                        }
                    }
                    None => break,
                },
                code = &mut self.exit, if exit_code.is_none() => {
                    exit_code = Some(code.unwrap_or(0));
                    // 子进程已退出：把读端剩余输出排空（空闲超时即止），再投递 exit。
                    loop {
                        let next = tokio::select! {
                            biased;
                            _ = self.cancel.changed() => return,
                            next = tokio::time::timeout(DRAIN_IDLE, self.chunks.recv()) => next,
                        };
                        match next {
                            Ok(Some(text)) => {
                                if !deliver(&listeners.data, text, &mut self.cancel).await {
                                    return;
                                }
                            }
                            _ => break,
                        }
                    }
                    break;
                }
            }
        }
        // 读端先 EOF 时，退出码可能还没到。
        let code = match exit_code {
            Some(code) => code,
            None => tokio::select! {
                biased;
                _ = self.cancel.changed() => return,
                code = &mut self.exit => code.unwrap_or(0),
            },
        };
        if !deliver(&listeners.exit, code, &mut self.cancel).await {
            return;
        }
        (self.on_exit)(&self.id);
        listeners.clear();
    }
}
