//! 单个串口路径的会话（S2）：状态机 + 打开/关闭 + 自动重连（`rust-u2-serial.md` §9）。
//!
//! 状态机：`closed → opening → open → closing → closed`，以及 `open → disconnected →（设备回来）→ opening`。
//! `disconnected` 与 `error` 的区别：前者等设备回来自动重连，后者是打开失败、需要用户重试。
//!
//! open/close/reconnect 在会话内**串行**（同一把异步锁），不同会话互不阻塞。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value, json};
use tokio::sync::Mutex as AsyncMutex;

use super::transport::{SerialConfig, SerialOpenError, SerialPort, SerialTransport};

/// TS `DEFAULT_SERIAL_SIGNALS`（S4 才会真正驱动 DTR/RTS，这里只用于状态投影）。
const DEFAULT_SIGNALS: (bool, bool) = (true, true);

#[derive(Clone)]
struct Status {
    state: &'static str,
    config: Option<SerialConfig>,
    error: Option<(String, String)>,
}

/// 一个串口路径的会话。由 `SerialService` 持有；同一连接内每个路径一个。
pub struct SerialSession {
    path: String,
    transport: Arc<dyn SerialTransport>,
    status: Mutex<Status>,
    port: Mutex<Option<Box<dyn SerialPort>>>,
    queue: AsyncMutex<()>,
    last_changed_at: AtomicU64,
    on_status: Arc<dyn Fn(Value) + Send + Sync>,
}

impl SerialSession {
    pub fn new(
        path: String,
        transport: Arc<dyn SerialTransport>,
        on_status: Arc<dyn Fn(Value) + Send + Sync>,
    ) -> Self {
        Self {
            path,
            transport,
            status: Mutex::new(Status {
                state: "closed",
                config: None,
                error: None,
            }),
            port: Mutex::new(None),
            queue: AsyncMutex::new(()),
            last_changed_at: AtomicU64::new(now_ms()),
            on_status,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn state(&self) -> &'static str {
        self.lock(&self.status).state
    }

    pub fn last_changed_at(&self) -> u64 {
        self.last_changed_at.load(Ordering::SeqCst)
    }

    /// TS `currentStatus`：`{state, path, signals, config?, error?}`。
    pub fn current_status(&self) -> Value {
        let status = self.lock(&self.status).clone();
        render_status(&self.path, &status)
    }

    /// TS `open`：已在打开同一配置时不重复打开；状态为 `disconnected` 时先收口再开。
    pub async fn open(&self, config: SerialConfig) -> Result<(), SerialOpenError> {
        let _guard = self.queue.lock().await;
        let already_open = {
            let status = self.lock(&self.status);
            status.state == "open" && status.config.as_ref() == Some(&config)
        };
        if already_open {
            return Ok(());
        }
        if self.lock(&self.port).is_some() || self.state() == "disconnected" {
            self.close_now();
        }
        self.open_port(config).await
    }

    /// TS `close`：幂等；关闭后状态回到 `closed`（保留 config 便于展示）。
    pub async fn close(&self) {
        let _guard = self.queue.lock().await;
        self.close_now();
    }

    /// 设备重新出现时由轮询调用：只有仍处于 `disconnected` 才重连。
    pub async fn reconnect(&self) -> Result<(), SerialOpenError> {
        let _guard = self.queue.lock().await;
        let (state, config) = {
            let status = self.lock(&self.status);
            (status.state, status.config.clone())
        };
        if state != "disconnected" {
            return Ok(());
        }
        let Some(config) = config else {
            return Ok(());
        };
        self.open_port(config).await
    }

    /// 设备消失（由轮询发现）：`autoReconnect` 为真进 `disconnected`，否则直接 `closed`。
    pub async fn handle_disconnect(&self) {
        let _guard = self.queue.lock().await;
        let config = self.lock(&self.status).config.clone();
        if let Some(port) = self.lock(&self.port).take() {
            port.close();
        }
        let auto_reconnect = config.as_ref().is_some_and(|config| config.auto_reconnect);
        self.set_status(
            if auto_reconnect {
                "disconnected"
            } else {
                "closed"
            },
            config,
            None,
        );
    }

    async fn open_port(&self, config: SerialConfig) -> Result<(), SerialOpenError> {
        self.set_status("opening", Some(config.clone()), None);
        match self.transport.open(&self.path, &config).await {
            Ok(port) => {
                *self.lock(&self.port) = Some(port);
                self.set_status("open", Some(config), None);
                Ok(())
            }
            Err(error) => {
                let mapped = if error.code == "io" {
                    SerialOpenError::from_message(error.message)
                } else {
                    error
                };
                self.set_status(
                    "error",
                    Some(config),
                    Some((mapped.code.to_owned(), mapped.message.clone())),
                );
                Err(mapped)
            }
        }
    }

    fn close_now(&self) {
        let port = self.lock(&self.port).take();
        let config = self.lock(&self.status).config.clone();
        if port.is_some() {
            self.set_status("closing", config.clone(), None);
            if let Some(port) = port {
                port.close();
            }
            self.set_status("closed", config, None);
            return;
        }
        if self.state() != "closed" {
            self.set_status("closed", config, None);
        }
    }

    fn set_status(
        &self,
        state: &'static str,
        config: Option<SerialConfig>,
        error: Option<(String, String)>,
    ) {
        {
            let mut status = self.lock(&self.status);
            status.state = state;
            status.config = config;
            status.error = error;
        }
        self.last_changed_at.store(now_ms(), Ordering::SeqCst);
        let status = self.lock(&self.status).clone();
        (self.on_status)(render_status(&self.path, &status));
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn render_status(path: &str, status: &Status) -> Value {
    let mut value = Map::new();
    value.insert("state".to_owned(), Value::from(status.state));
    value.insert("path".to_owned(), Value::from(path.to_owned()));
    value.insert(
        "signals".to_owned(),
        json!({"dtr": DEFAULT_SIGNALS.0, "rts": DEFAULT_SIGNALS.1}),
    );
    if let Some(config) = &status.config {
        value.insert("config".to_owned(), config.to_json());
    }
    if let Some((code, message)) = &status.error {
        value.insert(
            "error".to_owned(),
            json!({"code": code, "message": message}),
        );
    }
    Value::Object(value)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
