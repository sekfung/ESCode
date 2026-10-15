//! 一个已启动的运行时实例：链路 + 世代 + 帧泵（读端唯一消费者）。
//!
//! 帧泵的分流（每行出站 NDJSON）：
//!
//! | 帧 | 去向 |
//! | --- | --- |
//! | 应答（`id` 命中在飞请求） | 交给 [`Instance::call`] 的等待方 |
//! | 反向请求（有 `method` 与 `id`，`id` 不是在飞请求） | Host 应答器（`HostResponder`）应答后写回 runtime |
//! | 其余（通知/事件帧） | 扇出给全部 [`FrameSubscription`] |
//!
//! 读端关闭（实例退出/崩溃/收口）时：在飞请求**立即**失败（此前要等到各自的超时）、
//! 订阅者收到流结束、发布一次 `unavailable` 生命周期事件。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::host_io::{
    HostError, HostRequest, LifecycleEvent, LifecycleHub, ResponderSlot, RuntimeState,
};
use crate::link;

/// 订阅者通道容量。溢出策略：**不静默丢帧**——满即摘除该订阅者并置 `lagged`，
/// 接收端读到流结束后可据 [`FrameSubscription::lagged`] 区分「实例退出」与「消费过慢」
/// （V4 面据此让该连接的订阅失效、由客户端 resync 恢复）。
const SUBSCRIBER_CAPACITY: usize = 4096;

/// 首个订阅者出现之前的帧缓冲上限。帧泵在实例登记时就启动（反向请求必须有人应答），
/// 旧实现读循环随首个订阅才启动、runtime 的早期帧（`startup/storageState` 等）留在管道里不丢；
/// 这里用有界缓冲保持该保证：首个订阅者先收到缓冲帧，超限则该订阅者一开始就是 lagged。
const EARLY_FRAMES_CAPACITY: usize = 1024;

/// 实例出站流的一个订阅。
pub struct FrameSubscription {
    rx: mpsc::Receiver<String>,
    lagged: Arc<AtomicBool>,
}

impl FrameSubscription {
    /// 下一帧；`None` = 实例读端已关闭或本订阅因溢出被摘除（见 [`Self::lagged`]）。
    pub async fn recv(&mut self) -> Option<String> {
        self.rx.recv().await
    }

    /// 是否因消费过慢被摘除。
    pub fn lagged(&self) -> bool {
        self.lagged.load(Ordering::SeqCst)
    }
}

struct Subscriber {
    tx: mpsc::Sender<String>,
    lagged: Arc<AtomicBool>,
}

/// 帧泵与请求方共享的状态（帧泵持 Arc，不需要自引用）。
pub(crate) struct InstanceState {
    pending: std::sync::Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<Value>>>,
    subscribers: std::sync::Mutex<Vec<Subscriber>>,
    /// 首个订阅者之前的缓冲（`None` = 已交给首个订阅者）；`bool` = 是否溢出。
    early: std::sync::Mutex<Option<(Vec<String>, bool)>>,
    requests: AtomicU64,
    ready: AtomicBool,
    /// 读端已关闭：此后的 `call` 立即失败。
    closed: AtomicBool,
    writer: tokio::sync::Mutex<link::RuntimeWriter>,
    responder: ResponderSlot,
    lifecycle: Arc<LifecycleHub>,
    event: LifecycleEvent,
}

impl InstanceState {
    fn lock_pending(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<String, tokio::sync::oneshot::Sender<Value>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_subscribers(&self) -> std::sync::MutexGuard<'_, Vec<Subscriber>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn send_line(&self, line: &str) -> Result<()> {
        self.writer.lock().await.send_line(line).await
    }

    fn lock_early(&self) -> std::sync::MutexGuard<'_, Option<(Vec<String>, bool)>> {
        self.early
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 扇出一帧：满的订阅者摘除并置 lagged，已关闭的直接摘除；尚无订阅者时进早期缓冲。
    fn fan_out(&self, line: &str) {
        {
            let mut early = self.lock_early();
            if let Some((frames, overflowed)) = early.as_mut() {
                if frames.len() < EARLY_FRAMES_CAPACITY {
                    frames.push(line.to_owned());
                } else {
                    *overflowed = true;
                }
                return;
            }
        }
        self.lock_subscribers().retain(|subscriber| {
            match subscriber.tx.try_send(line.to_owned()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    subscriber.lagged.store(true, Ordering::SeqCst);
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
    }

    /// 读端关闭的收尾：先置 closed 再清在飞请求（与 `call` 的「先登记再查 closed」配对，
    /// 两种交错下等待方都会立即失败）；订阅者随发送端 drop 收到流结束。
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.lock_pending().clear();
        self.lock_subscribers().clear();
        let mut event = self.event.clone();
        event.state = RuntimeState::Unavailable;
        self.lifecycle.publish(&event);
    }
}

/// 一个已启动的运行时实例。
pub struct Instance {
    pub identity: String,
    pub generation: u64,
    lifecycle: tokio::sync::Mutex<link::Lifecycle>,
    state: Arc<InstanceState>,
}

impl Instance {
    /// 建实例并立即启动帧泵（读端唯一消费者），随后发布 `available`。
    pub(crate) fn start(
        runtime: link::RuntimeLink,
        event: LifecycleEvent,
        responder: ResponderSlot,
        lifecycle_hub: Arc<LifecycleHub>,
    ) -> Arc<Self> {
        let link::RuntimeLink {
            reader,
            writer,
            lifecycle,
        } = runtime;
        let state = Arc::new(InstanceState {
            pending: Default::default(),
            subscribers: Default::default(),
            early: std::sync::Mutex::new(Some((Vec::new(), false))),
            requests: AtomicU64::new(1),
            ready: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            writer: tokio::sync::Mutex::new(writer),
            responder,
            lifecycle: lifecycle_hub,
            event: event.clone(),
        });
        let instance = Arc::new(Self {
            identity: event.identity.clone(),
            generation: event.generation,
            lifecycle: tokio::sync::Mutex::new(lifecycle),
            state: Arc::clone(&state),
        });
        tokio::spawn(pump(reader, state));
        instance.state.lifecycle.publish(&event);
        instance
    }

    /// 实例是否仍在运行。同步判定：生命周期锁在收口/等待期间被持有，
    /// 此时按存活处理（真正的关闭会在下一次读写上暴露）。
    pub fn is_alive(&self) -> bool {
        if self.state.closed.load(Ordering::SeqCst) {
            return false;
        }
        match self.lifecycle.try_lock() {
            Ok(lifecycle) => lifecycle.is_alive(),
            Err(_) => true,
        }
    }

    /// 写入一行请求帧（只竞争写锁）。
    pub async fn send_line(&self, line: &str) -> Result<()> {
        self.state.send_line(line).await
    }

    /// 订阅出站帧（每个消费者一个）。首个订阅者先收到登记前缓冲的早期帧。
    pub async fn subscribe(&self) -> FrameSubscription {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_CAPACITY);
        let lagged = Arc::new(AtomicBool::new(false));
        // 早期缓冲与登记在同一把锁下完成：帧泵此后的帧一定走订阅者，不会插到缓冲帧之前。
        let mut early = self.state.lock_early();
        if let Some((frames, overflowed)) = early.take() {
            if overflowed {
                lagged.store(true, Ordering::SeqCst);
            }
            for frame in frames {
                let _ = tx.try_send(frame);
            }
        }
        if !self.state.closed.load(Ordering::SeqCst) && !lagged.load(Ordering::SeqCst) {
            self.state.lock_subscribers().push(Subscriber {
                tx,
                lagged: Arc::clone(&lagged),
            });
        }
        drop(early);
        FrameSubscription { rx, lagged }
    }

    /// 等运行时报到（`startup/storageState` phase=ready）；实例退出即提前返回 false。
    pub async fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.state.ready.load(Ordering::SeqCst) {
                return true;
            }
            if !self.is_alive() || tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 向 runtime 发一条请求并等应答（按 id 关联；`timeout` 到点即失败）。
    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.call_with(method, params, None, timeout).await
    }

    /// 同 [`Self::call`]，并在信封上附 `trace`（TS `client.request(..., {trace})` 同形）。
    pub async fn call_with(
        &self,
        method: &str,
        params: Value,
        trace: Option<Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let id = format!(
            "host-{}",
            self.state.requests.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.state.lock_pending().insert(id.clone(), tx);
        if self.state.closed.load(Ordering::SeqCst) {
            self.state.lock_pending().remove(&id);
            anyhow::bail!("runtime closed while awaiting `{method}`");
        }
        let mut envelope = json!({"id": id, "method": method, "params": params});
        if let Some(trace) = trace {
            envelope["trace"] = trace;
        }
        if let Err(error) = self.send_line(&envelope.to_string()).await {
            self.state.lock_pending().remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => anyhow::bail!("runtime closed while awaiting `{method}`"),
            Err(_) => {
                self.state.lock_pending().remove(&id);
                anyhow::bail!("runtime did not answer `{method}` within {timeout:?}")
            }
        }
    }

    /// 请求收口并等待（宽限内）。
    pub async fn shutdown(&self, grace: Duration) -> Vec<String> {
        let mut lifecycle = self.lifecycle.lock().await;
        lifecycle.cancel();
        match lifecycle.wait(grace).await {
            Ok(residual) => residual,
            Err(error) => vec![format!("{}: {error}", self.identity)],
        }
    }
}

/// 帧泵：读端唯一消费者。
async fn pump(mut reader: link::RuntimeReader, state: Arc<InstanceState>) {
    while let Some(line) = reader.line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(parsed) = serde_json::from_str::<Value>(trimmed).ok() else {
            state.fan_out(trimmed);
            continue;
        };
        if parsed["method"] == "startup/storageState" && parsed["params"]["phase"] == "ready" {
            state.ready.store(true, Ordering::SeqCst);
        }
        let reply_id = parsed["id"].as_str().map(str::to_owned);
        if let Some(waiter) = reply_id
            .as_ref()
            .filter(|_| parsed.get("method").is_none())
            .and_then(|id| state.lock_pending().remove(id))
        {
            let _ = waiter.send(parsed);
            continue;
        }
        if parsed.get("method").is_some_and(Value::is_string) && !parsed["id"].is_null() {
            answer_host_request(&state, parsed);
            continue;
        }
        state.fan_out(trimmed);
    }
    state.close();
}

/// 应答一条反向请求（独立任务：慢应答不阻塞帧泵）。
fn answer_host_request(state: &Arc<InstanceState>, request: Value) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let id = request["id"].clone();
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let outcome = match state.responder.current() {
            Some(responder) => {
                responder(HostRequest {
                    identity: state.event.identity.clone(),
                    workspace_path: state.event.workspace_path.clone(),
                    method: method.clone(),
                    params: request["params"].clone(),
                })
                .await
            }
            None => Err(HostError::unsupported(&method)),
        };
        let response = match outcome {
            Ok(result) => json!({"id": id, "result": result}),
            Err(error) => {
                json!({"id": id, "error": {"code": error.code, "message": error.message}})
            }
        };
        // 发送失败 = 实例已关闭；等待方随实例退出一并失败，这里无需再上报。
        let _ = state.send_line(&response.to_string()).await;
    });
}
