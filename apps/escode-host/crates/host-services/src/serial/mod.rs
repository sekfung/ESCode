//! `serial` 域（docs/specs/rust-u2-serial.md）。
//!
//! **S1 已交付**：端口枚举 + 轮询 + `onPorts`（`list` / `setWatching` / `onPorts`）。
//! S2–S4（会话、数据面、信号与循环）待做——那是同一个域的另外三片。
//!
//! 作用域：**每连接（≈每窗口）**（用户决定 2026-10-17）。串口会话与 `seq` 都属于连接，
//! 因此本服务由连接级 channel 持有，而不是 Host 级单例。

pub mod enumerate;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use enumerate::PortEnumerator;

/// TS `DEFAULT_POLL_INTERVAL_MS`。
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 1500;

struct Inner {
    watching: Mutex<bool>,
    disposed: Mutex<bool>,
    /// 上一次已广播的端口路径键（`path` 以 `\n` 拼接）。
    last_ports_key: Mutex<Option<String>>,
    listeners: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
    poll_task: Mutex<Option<JoinHandle<()>>>,
}

/// 连接内唯一的串口服务。
#[derive(Clone)]
pub struct SerialService {
    enumerator: Arc<dyn PortEnumerator>,
    poll_interval: Duration,
    inner: Arc<Inner>,
}

impl SerialService {
    pub fn new(enumerator: Arc<dyn PortEnumerator>) -> Self {
        Self {
            enumerator,
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            inner: Arc::new(Inner {
                watching: Mutex::new(false),
                disposed: Mutex::new(false),
                last_ports_key: Mutex::new(None),
                listeners: Mutex::new(Vec::new()),
                poll_task: Mutex::new(None),
            }),
        }
    }

    /// 测试用：自定义轮询间隔。
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// 订阅 `onPorts`（每个订阅者一条独立流）。
    pub fn subscribe_ports(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock(&self.inner.listeners).push(tx);
        rx
    }

    /// TS `list`：枚举当前端口；失败即 `io` 错误（不返回空列表）。
    pub async fn list(&self) -> Result<Vec<enumerate::PortInfo>, String> {
        self.enumerator.list().await
    }

    /// TS `setWatching`：切轮询开关；开启时**清空 dedupe 键**并立即轮询一次。
    pub async fn set_watching(&self, watching: bool) {
        *self.lock(&self.inner.watching) = watching;
        if watching {
            *self.lock(&self.inner.last_ports_key) = None;
        }
        self.update_polling();
    }

    /// 停止轮询（连接关闭时由 channel 的 close 钩子调用）。
    pub fn shutdown(&self) {
        *self.lock(&self.inner.disposed) = true;
        self.stop_polling();
        self.lock(&self.inner.listeners).clear();
    }

    /// TS `shouldPoll`：本片只有 `watching`（会话断开重连随 S2 接入）。
    fn should_poll(&self) -> bool {
        !*self.lock(&self.inner.disposed) && *self.lock(&self.inner.watching)
    }

    fn update_polling(&self) {
        if !self.should_poll() {
            self.stop_polling();
            return;
        }
        let mut task = self.lock(&self.inner.poll_task);
        if task.is_none() {
            let service = self.clone();
            *task = Some(tokio::spawn(async move {
                // 立即跑一次，再按间隔续跑（TS `schedulePoll(0)`）。
                loop {
                    if !service.should_poll() {
                        break;
                    }
                    service.poll_once().await;
                    tokio::time::sleep(service.poll_interval).await;
                }
                let mut slot = service.lock(&service.inner.poll_task);
                *slot = None;
            }));
        }
    }

    fn stop_polling(&self) {
        if let Some(handle) = self.lock(&self.inner.poll_task).take() {
            handle.abort();
        }
    }

    /// TS `pollOnce`：枚举 → 与上次的 `path` 键比较 → 变了才广播。
    /// 失败只记日志，不广播、不改变 dedupe 键。
    async fn poll_once(&self) {
        let Ok(ports) = self.enumerator.list().await else {
            return;
        };
        let key = enumerate::ports_key(&ports);
        let changed = {
            let mut last = self.lock(&self.inner.last_ports_key);
            if self.watching() && last.as_deref() != Some(&key) {
                *last = Some(key);
                true
            } else {
                false
            }
        };
        if !changed {
            return;
        }
        let payload = Value::Array(ports.into_iter().map(Value::Object).collect());
        self.lock(&self.inner.listeners)
            .retain(|tx| tx.send(payload.clone()).is_ok());
    }

    fn watching(&self) -> bool {
        *self.lock(&self.inner.watching)
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
