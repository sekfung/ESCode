//! `serial` 域（docs/specs/rust-u2-serial.md）。
//!
//! **S1–S3 已交付**：枚举/轮询/`onPorts`；会话（含自动重连）；数据面（`write` / `clear` / `onData`）。
//! **S4**：信号（`setSignals` + 复位脉冲）与定时循环（`startLoop` / `stopLoop`）。
//!
//! 作用域：**每连接（≈每窗口）**（用户决定 2026-10-17）。串口会话与 `seq` 都属于连接，
//! 因此本服务由连接级 channel 持有，而不是 Host 级单例。

pub mod buffer;
pub mod data;
pub mod enumerate;
pub mod loop_runner;
pub mod session;
pub mod session_status;
pub mod signals;
pub mod transport;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use enumerate::PortEnumerator;
use session::SerialSession;
use signals::{Pulse, Signals};
use transport::{SerialConfig, SerialOpenError, SerialTransport};

/// TS `DEFAULT_POLL_INTERVAL_MS`。
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 1500;
/// TS `SERIAL_MAX_ACTIVE_SESSIONS`：每连接（≈每窗口）同时活动的会话上限。
pub const MAX_ACTIVE_SESSIONS: usize = 4;
/// TS `MAX_RETAINED_SESSIONS`：保留的会话条目上限（关掉后还给快照）。
const MAX_RETAINED_SESSIONS: usize = 8;

struct Inner {
    watching: Mutex<bool>,
    disposed: Mutex<bool>,
    /// 上一次已广播的端口路径键（`path` 以 `\n` 拼接）。
    last_ports_key: Mutex<Option<String>>,
    ports_listeners: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
    status_listeners: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
    data_listeners: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
    poll_task: Mutex<Option<JoinHandle<()>>>,
    sessions: Mutex<BTreeMap<String, Arc<SerialSession>>>,
    /// 连接内的统一 seq（跨会话、跨关闭重开单调递增；由数据面分配）。
    seq: Arc<AtomicU64>,
}

/// 连接内唯一的串口服务。
#[derive(Clone)]
pub struct SerialService {
    enumerator: Arc<dyn PortEnumerator>,
    transport: Arc<dyn SerialTransport>,
    poll_interval: Duration,
    inner: Arc<Inner>,
}

impl SerialService {
    pub fn new(enumerator: Arc<dyn PortEnumerator>, transport: Arc<dyn SerialTransport>) -> Self {
        Self {
            enumerator,
            transport,
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            inner: Arc::new(Inner {
                watching: Mutex::new(false),
                disposed: Mutex::new(false),
                last_ports_key: Mutex::new(None),
                ports_listeners: Mutex::new(Vec::new()),
                status_listeners: Mutex::new(Vec::new()),
                data_listeners: Mutex::new(Vec::new()),
                poll_task: Mutex::new(None),
                sessions: Mutex::new(BTreeMap::new()),
                seq: Arc::new(AtomicU64::new(0)),
            }),
        }
    }

    /// 测试用：自定义轮询间隔。
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub fn subscribe_ports(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock(&self.inner.ports_listeners).push(tx);
        rx
    }

    pub fn subscribe_status(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock(&self.inner.status_listeners).push(tx);
        rx
    }

    pub fn subscribe_data(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock(&self.inner.data_listeners).push(tx);
        rx
    }

    /// TS `list`：枚举当前端口；失败即 `io` 错误（不返回空列表）。
    pub async fn list(&self) -> Result<Vec<enumerate::PortInfo>, String> {
        self.enumerator.list().await
    }

    /// TS `setWatching`：切轮询开关；开启时清空 dedupe 键并立即轮询一次。
    pub async fn set_watching(&self, watching: bool) {
        *self.lock(&self.inner.watching) = watching;
        if watching {
            *self.lock(&self.inner.last_ports_key) = None;
        }
        self.update_polling();
    }

    /// TS `open`：超过活动上限直接拒绝；否则取或建会话再打开。
    pub async fn open(&self, path: &str, config: &Value) -> Result<(), SerialOpenError> {
        let parsed = SerialConfig::from_json(config)?;
        {
            let sessions = self.lock(&self.inner.sessions);
            let active_others = sessions
                .values()
                .filter(|session| session.path() != path && session.state() != "closed")
                .count();
            if active_others >= MAX_ACTIVE_SESSIONS {
                return Err(SerialOpenError::new(
                    "invalidInput",
                    format!(
                        "At most {MAX_ACTIVE_SESSIONS} serial ports can be open at the same time"
                    ),
                ));
            }
        }
        let session = self.session_for(path);
        session.open(parsed).await?;
        self.evict_closed_sessions();
        Ok(())
    }

    /// TS `close`：不存在的路径是空操作。
    pub async fn close(&self, path: &str) {
        let session = self.lock(&self.inner.sessions).get(path).cloned();
        if let Some(session) = session {
            session.close().await;
        }
    }

    /// TS `write`：会话不存在或未打开 → `notOpen`；返回本次写入的 seq。
    pub async fn write(
        &self,
        path: &str,
        bytes: Vec<u8>,
        source: &str,
        session_id: Option<String>,
    ) -> Result<u64, SerialOpenError> {
        let session = self.lock(&self.inner.sessions).get(path).cloned();
        let Some(session) = session else {
            return Err(SerialOpenError::new(
                "notOpen",
                "Serial port is not open".to_owned(),
            ));
        };
        session
            .write(bytes, source, session_id)
            .await
            .map_err(|error| SerialOpenError::new(error.code, error.message))
    }

    /// TS `clear`：未知路径是空操作。
    pub fn clear(&self, path: &str) {
        let session = self.lock(&self.inner.sessions).get(path).cloned();
        if let Some(session) = session {
            session.clear();
        }
    }

    /// TS `getSnapshot`：未知路径回 `closed` 空快照。
    pub fn get_snapshot(&self, path: &str) -> Value {
        let session = self.lock(&self.inner.sessions).get(path).cloned();
        match session {
            Some(session) => session.snapshot(),
            None => serde_json::json!({
                "status": {"state": "closed", "path": path},
                "chunks": [],
                "seq": self.inner.seq.load(Ordering::SeqCst),
                "stats": {"rxBytes": 0, "txBytes": 0},
            }),
        }
    }

    /// TS `listSessions`：只回活动（非 `closed`）的会话。
    pub fn list_sessions(&self) -> Vec<Value> {
        self.lock(&self.inner.sessions)
            .values()
            .filter(|session| session.state() != "closed")
            .map(|session| {
                serde_json::json!({"path": session.path(), "status": session.current_status()})
            })
            .collect()
    }

    /// 停止轮询、关闭所有会话（停循环、关端口）并清空监听（连接关闭时由 channel 的 close 钩子调用）。
    /// 对应 TS `disposeAllAndWait`：不关闭会话的话，循环任务会一直持有会话继续写。
    pub async fn shutdown(&self) {
        *self.lock(&self.inner.disposed) = true;
        self.stop_polling();
        let sessions: Vec<Arc<SerialSession>> =
            self.lock(&self.inner.sessions).values().cloned().collect();
        for session in sessions {
            session.close().await;
        }
        self.lock(&self.inner.ports_listeners).clear();
        self.lock(&self.inner.status_listeners).clear();
        self.lock(&self.inner.data_listeners).clear();
    }

    /// 取会话；未知路径 → `notOpen`。
    fn session(&self, path: &str) -> Result<Arc<SerialSession>, SerialOpenError> {
        self.lock(&self.inner.sessions)
            .get(path)
            .cloned()
            .ok_or_else(|| SerialOpenError::new("notOpen", "Serial port is not open".to_owned()))
    }

    /// 取或建会话；新会话的状态变化会广播到 `onStatus`。
    fn session_for(&self, path: &str) -> Arc<SerialSession> {
        if let Some(session) = self.lock(&self.inner.sessions).get(path).cloned() {
            return session;
        }
        let inner = Arc::clone(&self.inner);
        // TS `{...status, path}`：状态本身已含 path，直接广播完整对象。
        let on_status: Arc<dyn Fn(Value) + Send + Sync> = Arc::new(move |status: Value| {
            inner
                .status_listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|tx| tx.send(status.clone()).is_ok());
        });
        let inner_for_chunk = Arc::clone(&self.inner);
        let path_for_chunk = path.to_owned();
        let on_chunk: Arc<dyn Fn(buffer::Chunk) + Send + Sync> = Arc::new(move |chunk| {
            let payload = chunk.to_json(&path_for_chunk);
            inner_for_chunk
                .data_listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|tx| tx.send(payload.clone()).is_ok());
        });
        let session = Arc::new(SerialSession::new(
            path.to_owned(),
            Arc::clone(&self.transport),
            Arc::clone(&self.inner.seq),
            on_status,
            on_chunk,
        ));
        self.lock(&self.inner.sessions)
            .insert(path.to_owned(), Arc::clone(&session));
        session
    }

    /// TS `evictClosedSessions`：超出保留上限时按 `lastChangedAt` 淘汰最早的已关闭会话。
    fn evict_closed_sessions(&self) {
        let mut closed: Vec<(String, u64)> = {
            let sessions = self.lock(&self.inner.sessions);
            if sessions.len() <= MAX_RETAINED_SESSIONS {
                return;
            }
            sessions
                .values()
                .filter(|session| session.state() == "closed")
                .map(|session| (session.path().to_owned(), session.last_changed_at()))
                .collect()
        };
        closed.sort_by_key(|(_, changed)| *changed);
        for (path, _) in closed {
            if self.lock(&self.inner.sessions).len() <= MAX_RETAINED_SESSIONS {
                return;
            }
            self.lock(&self.inner.sessions).remove(&path);
        }
    }

    /// TS `shouldPoll`：`watching` 或有会话在等重连。
    fn should_poll(&self) -> bool {
        if *self.lock(&self.inner.disposed) {
            return false;
        }
        if *self.lock(&self.inner.watching) {
            return true;
        }
        self.lock(&self.inner.sessions)
            .values()
            .any(|session| session.state() == "disconnected")
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

    /// TS `pollOnce`：枚举 → 端口键变化才广播 → 断开检测与重连。
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
        if changed {
            let payload = Value::Array(
                ports
                    .iter()
                    .map(|port| Value::Object(port.clone()))
                    .collect(),
            );
            self.lock(&self.inner.ports_listeners)
                .retain(|tx| tx.send(payload.clone()).is_ok());
        }

        // 断开检测：打开着的会话其路径从枚举里消失 → 按 autoReconnect 进 disconnected/closed。
        // 登记差异：TS 由驱动的 close 事件即时发现，这里靠轮询（最多晚一个间隔）。
        let present: Vec<String> = ports
            .iter()
            .filter_map(|port| port["path"].as_str().map(str::to_owned))
            .collect();
        let sessions: Vec<Arc<SerialSession>> =
            self.lock(&self.inner.sessions).values().cloned().collect();
        for session in &sessions {
            let path = session.path().to_owned();
            if session.state() == "open" && !present.iter().any(|candidate| candidate == &path) {
                session.handle_disconnect().await;
            }
        }
        for session in &sessions {
            if session.state() == "disconnected" && present.iter().any(|c| c == session.path()) {
                let _ = session.reconnect().await;
            }
        }
    }

    fn watching(&self) -> bool {
        *self.lock(&self.inner.watching)
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[path = "service_signals.rs"]
mod service_signals;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mod_tests_service.rs"]
mod service_tests;
