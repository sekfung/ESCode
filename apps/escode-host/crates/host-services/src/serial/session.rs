//! 单个串口路径的会话（S2–S4）：状态机、打开/关闭、自动重连、数据面与信号/循环
//! （`rust-u2-serial.md` §9–§12）。
//!
//! 状态机：`closed → opening → open → closing → closed`，以及 `open → disconnected →（设备回来）→ opening`。
//! `disconnected` 与 `error` 的区别：前者等设备回来自动重连，后者是打开失败、需要用户重试。
//!
//! open/close/reconnect/setSignals 在会话内**串行**（同一把异步锁），不同会话互不阻塞；
//! 写入与循环 tick 不进队列（TS 同）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use super::buffer::Chunk;
use super::data::{DataSink, WRITE_LIMIT_BYTES};
use super::loop_runner::{self, LoopState, WriteFn};
use super::session_status::{Status, now_ms, render_status};
use super::signals::{self, DEFAULT_SIGNALS, Signals};
use super::transport::{SerialConfig, SerialIoError, SerialOpenError, SerialPort, SerialTransport};

/// 一个串口路径的会话。由 `SerialService` 持有；同一连接内每个路径一个。
pub struct SerialSession {
    path: String,
    transport: Arc<dyn SerialTransport>,
    status: Mutex<Status>,
    port: Mutex<Option<Arc<dyn SerialPort>>>,
    /// DTR/RTS 当前输出；重连后按断开前的状态恢复。
    signals: Mutex<Signals>,
    /// 进行中的循环发送；`None` 表示没有循环。
    looping: Mutex<Option<Arc<LoopState>>>,
    loop_generation: AtomicU64,
    queue: AsyncMutex<()>,
    last_changed_at: AtomicU64,
    on_status: Arc<dyn Fn(Value) + Send + Sync>,
    data: Arc<DataSink>,
}

impl SerialSession {
    pub fn new(
        path: String,
        transport: Arc<dyn SerialTransport>,
        seq: Arc<AtomicU64>,
        on_status: Arc<dyn Fn(Value) + Send + Sync>,
        on_chunk: Arc<dyn Fn(Chunk) + Send + Sync>,
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
            signals: Mutex::new(DEFAULT_SIGNALS),
            looping: Mutex::new(None),
            loop_generation: AtomicU64::new(0),
            queue: AsyncMutex::new(()),
            last_changed_at: AtomicU64::new(now_ms()),
            on_status,
            data: DataSink::new(seq, on_chunk),
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

    /// TS `currentStatus`：`{state, path, signals, config?, error?, loop?}`。
    pub fn current_status(&self) -> Value {
        self.status_value()
    }

    /// TS `write`：64KB 上限、空写回当前 seq；先记 TX 再真正写出（seq 先于设备回显）。
    pub async fn write(
        &self,
        bytes: Vec<u8>,
        source: &str,
        session_id: Option<String>,
    ) -> Result<u64, SerialIoError> {
        if self.state() != "open" {
            return Err(not_open());
        }
        if bytes.len() > WRITE_LIMIT_BYTES {
            return Err(SerialIoError::new(
                "invalidInput",
                format!("Write exceeds {WRITE_LIMIT_BYTES} bytes: {}", bytes.len()),
            ));
        }
        if bytes.is_empty() {
            return Ok(self.data.last_seq());
        }
        let seq = self.data.push_tx(source, bytes.clone(), session_id);
        // 锁内只取句柄，随后释放锁再 await（否则 future 不 Send）。
        let port = self.lock(&self.port).clone();
        let Some(port) = port else {
            return Err(not_open());
        };
        port.write(bytes).await?;
        Ok(seq)
    }

    /// TS `setSignals`：与 open/close 同队列串行；脉冲结束后恢复脉冲前状态。
    pub async fn set_signals(
        &self,
        dtr: Option<bool>,
        rts: Option<bool>,
        pulse: Option<signals::Pulse>,
    ) -> Result<Signals, SerialIoError> {
        let _guard = self.queue.lock().await;
        if self.state() != "open" || self.lock(&self.port).is_none() {
            return Err(not_open());
        }
        let flow_control = self
            .lock(&self.status)
            .config
            .as_ref()
            .is_some_and(|config| config.rtscts);
        if rts.is_some() && flow_control {
            return Err(SerialIoError::new(
                "invalidInput",
                "RTS is controlled by RTS/CTS flow control".to_owned(),
            ));
        }
        let previous = *self.lock(&self.signals);
        if let Some(pulse) = pulse {
            for step in signals::pulse_steps(pulse, previous) {
                self.apply_signals(step.signals).await?;
                if !step.hold.is_zero() {
                    tokio::time::sleep(step.hold).await;
                }
            }
            // 脉冲结束后恢复脉冲前的开关状态。
            self.apply_signals(previous).await?;
        } else {
            self.apply_signals(Signals {
                dtr: dtr.unwrap_or(previous.dtr),
                rts: rts.unwrap_or(previous.rts),
            })
            .await?;
        }
        let applied = *self.lock(&self.signals);
        self.publish_status();
        Ok(applied)
    }

    /// TS `startLoop`：参数同步校验；立即发第一次；再次启动静默替换旧任务。
    pub fn start_loop(
        self: &Arc<Self>,
        bytes: Vec<u8>,
        interval_ms: u64,
        count: Option<u64>,
    ) -> Result<(), SerialIoError> {
        if self.state() != "open" || self.lock(&self.port).is_none() {
            return Err(not_open());
        }
        if !loop_runner::is_valid_params(interval_ms, bytes.len(), count)
            || bytes.len() > WRITE_LIMIT_BYTES
        {
            return Err(SerialIoError::new(
                "invalidInput",
                "Invalid loop parameters".to_owned(),
            ));
        }
        let generation = self.loop_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let state = LoopState::new(generation, interval_ms, count);
        if let Some(previous) = self.lock(&self.looping).replace(Arc::clone(&state)) {
            previous.stop();
        }
        // TS 在 runner.start() 里先推一条 sent=0 的进度。
        self.publish_status();

        let write: WriteFn = {
            let session = Arc::clone(self);
            Arc::new(move |bytes| {
                let session = Arc::clone(&session);
                Box::pin(async move { session.write(bytes, "user", None).await.is_ok() })
            })
        };
        let on_progress: loop_runner::Callback = {
            let session = Arc::clone(self);
            Arc::new(move |generation| session.loop_progress(generation))
        };
        let on_finished: loop_runner::Callback = {
            let session = Arc::clone(self);
            Arc::new(move |generation| session.finish_loop(generation))
        };
        loop_runner::spawn(state, bytes, write, on_progress, on_finished);
        Ok(())
    }

    /// TS `stopLoop`：没有进行中的循环时无事件；有则停止并广播（不带 loop）。
    pub fn stop_loop(&self) {
        let state = self.lock(&self.looping).take();
        let Some(state) = state else {
            return;
        };
        state.stop();
        self.publish_status();
    }

    /// TS `clear`：只清缓冲与计数，不影响端口状态。
    pub fn clear(&self) {
        self.data.clear();
    }

    /// TS `snapshot`：状态 + 缓冲。
    pub fn snapshot(&self) -> Value {
        let mut snapshot = self.data.snapshot();
        snapshot["status"] = self.current_status();
        snapshot
    }

    pub fn last_seq(&self) -> u64 {
        self.data.last_seq()
    }

    async fn apply_signals(&self, signals: Signals) -> Result<(), SerialIoError> {
        let port = self.lock(&self.port).clone();
        let Some(port) = port else {
            return Err(not_open());
        };
        port.set_signals(signals).await?;
        *self.lock(&self.signals) = signals;
        Ok(())
    }

    fn set_status(
        &self,
        state: &'static str,
        config: Option<SerialConfig>,
        error: Option<(String, String)>,
    ) {
        // 离开 open（关闭、断开、出错）时循环发送随之停止。
        if state != "open" {
            self.stop_loop_silent();
        }
        {
            let mut status = self.lock(&self.status);
            status.state = state;
            status.config = config;
            status.error = error;
        }
        self.publish_status();
    }

    /// 广播当前状态（不改 state/config/error）。
    fn publish_status(&self) {
        self.last_changed_at.store(now_ms(), Ordering::SeqCst);
        (self.on_status)(self.status_value());
    }

    fn status_value(&self) -> Value {
        let status = self.lock(&self.status).clone();
        let signals = *self.lock(&self.signals);
        let looping = self
            .lock(&self.looping)
            .as_ref()
            .map(|state| state.to_json());
        render_status(&self.path, &status, signals, looping)
    }

    fn stop_loop_silent(&self) {
        if let Some(state) = self.lock(&self.looping).take() {
            state.stop();
        }
    }

    fn loop_progress(&self, generation: u64) {
        let current = self
            .lock(&self.looping)
            .as_ref()
            .is_some_and(|state| state.generation == generation);
        if current {
            self.publish_status();
        }
    }

    fn finish_loop(&self, generation: u64) {
        let mut slot = self.lock(&self.looping);
        if slot
            .as_ref()
            .is_none_or(|state| state.generation != generation)
        {
            return;
        }
        *slot = None;
        drop(slot);
        self.publish_status();
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for SerialSession {
    fn drop(&mut self) {
        if let Some(state) = self
            .looping
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            state.stop();
        }
        let port = self
            .port
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(port) = port {
            self.data.shutdown();
            port.close();
        }
    }
}

fn not_open() -> SerialIoError {
    SerialIoError::new("notOpen", "Serial port is not open".to_owned())
}

#[path = "session_open.rs"]
mod open;

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
