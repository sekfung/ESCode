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
//! 订阅者收到流结束、发布一次 `unavailable` 生命周期事件。不是 Host 请求的收口时（崩溃），
//! 帧泵随即回收实例（等实例结束、取残余清单、记 error 日志），见
//! docs/specs/rust-u2-host-resilience.md §2.2。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::closed::RuntimeClosed;
use crate::host_io::{LifecycleEvent, LifecycleHub, ResponderSlot, RuntimeState};
use crate::link;
use crate::request_timeout::RequestTimeout;
use pump::pump;

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

    /// 发送端是否已全部释放（实例读端关闭或本订阅被摘除）。已缓冲的帧仍可 `recv`。
    pub fn is_closed(&self) -> bool {
        self.rx.is_closed()
    }
}

/// 一代实例结束的记录（docs/specs/rust-u2-host-resilience.md §2.2 / §2.7）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitReport {
    pub generation: u64,
    /// true = Host 请求的收口（close/shutdown）；false = 崩溃（读端意外关闭）。
    pub requested: bool,
    /// 退出原因（embedded：实例任务结果；child：进程退出状态）。
    pub cause: String,
    /// 宽限后的残余任务/进程清单；验收口径为空。
    pub residual: Vec<String>,
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
    /// ready/closed 的状态通知（等待方不再 20 ms 轮询；见 `instance_ready.rs`）。
    ready: watch::Sender<ready::ReadyState>,
    /// 最近一次 `startup/storageState` 的完整 params（checking/ready/failed 及 migration 事实）。
    startup: watch::Sender<Option<Value>>,
    /// 读端已关闭：此后的 `call` 立即失败。
    closed: AtomicBool,
    writer: tokio::sync::Mutex<link::RuntimeWriter>,
    /// 链路生命周期（存活判定与收口）。帧泵的崩溃回收与 `shutdown` 共用这把锁。
    link_lifecycle: tokio::sync::Mutex<link::Lifecycle>,
    /// Host 已请求收口：此后读端关闭不算崩溃，帧泵不回收。
    stopping: AtomicBool,
    /// 收口/回收宽限（工具进程树 TERM→KILL 上限）。
    grace: Duration,
    /// 本代的结束记录（首次写入后不再改）。
    exit: watch::Sender<Option<ExitReport>>,
    responder: ResponderSlot,
    lifecycle: Arc<LifecycleHub>,
    event: LifecycleEvent,
    /// 生命周期公告相位：`available`/`unavailable` 成对且有序（见 [`Announce`]）。
    announce: std::sync::Mutex<Announce>,
}

/// 修复（评审 R1）：`available` 原先在 `Instance::start` 里、实例登记进槽位**之前**发布；监听方
/// （任务索引 syncer）立刻按 existing-only 取实例会扑空、进入 dormant，整个 workspace 不再同步。
/// 现在由实例表在登记后公告；实例若在公告前已退出，则两条都不发（TS 同：unavailable 只针对已
/// 公告过的运行时）。两次公告与读端关闭在同一把锁下判定，保证不会出现 unavailable 先于 available。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Announce {
    Pending,
    Available,
    Closed,
}

impl InstanceState {
    fn lock_pending(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<String, tokio::sync::oneshot::Sender<Value>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_announce(&self) -> std::sync::MutexGuard<'_, Announce> {
        self.announce
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

    async fn send_json(&self, value: &Value) -> Result<()> {
        self.writer.lock().await.send_value(value).await
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
        self.mark_closed();
        self.lock_pending().clear();
        self.lock_subscribers().clear();
        let mut phase = self.lock_announce();
        let announced = *phase == Announce::Available;
        *phase = Announce::Closed;
        if announced {
            let mut event = self.event.clone();
            event.state = RuntimeState::Unavailable;
            self.lifecycle.publish(&event);
        }
    }
}

// 帧泵与崩溃回收（子模块可直接访问本模块的私有状态）。
#[path = "instance_pump.rs"]
mod pump;

// ready/closed 通知（等待方不再轮询）。子模块与父模块共享 InstanceState 私有字段。
#[path = "instance_ready.rs"]
mod ready;

/// 一个已启动的运行时实例。
pub struct Instance {
    pub identity: String,
    pub generation: u64,
    state: Arc<InstanceState>,
}

impl Instance {
    /// 建实例并立即启动帧泵（读端唯一消费者），随后发布 `available`。
    pub(crate) fn start(
        runtime: link::RuntimeLink,
        event: LifecycleEvent,
        responder: ResponderSlot,
        lifecycle_hub: Arc<LifecycleHub>,
        grace: Duration,
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
            ready: watch::channel(ready::ReadyState::Pending).0,
            startup: watch::channel(None).0,
            closed: AtomicBool::new(false),
            writer: tokio::sync::Mutex::new(writer),
            link_lifecycle: tokio::sync::Mutex::new(lifecycle),
            stopping: AtomicBool::new(false),
            grace,
            exit: watch::channel(None).0,
            responder,
            lifecycle: lifecycle_hub,
            event: event.clone(),
            announce: std::sync::Mutex::new(Announce::Pending),
        });
        let instance = Arc::new(Self {
            identity: event.identity.clone(),
            generation: event.generation,
            state: Arc::clone(&state),
        });
        tokio::spawn(pump(reader, state));
        instance
    }

    /// 实例登记进槽位后由实例表调用：发布 `available`（读端已关闭则不发）。
    pub(crate) fn announce_available(&self) {
        let mut phase = self.state.lock_announce();
        if *phase == Announce::Pending {
            *phase = Announce::Available;
            self.state.lifecycle.publish(&self.state.event);
        }
    }

    /// 实例是否仍在运行。同步判定：生命周期锁在收口/等待期间被持有，
    /// 此时按存活处理（真正的关闭会在下一次读写上暴露）。
    pub fn is_alive(&self) -> bool {
        if self.state.closed.load(Ordering::SeqCst) {
            return false;
        }
        match self.state.link_lifecycle.try_lock() {
            Ok(lifecycle) => lifecycle.is_alive(),
            Err(_) => true,
        }
    }

    /// 该实例的 workspace 路径（与启动时的 `LifecycleEvent` 同源；不 realpath）。
    pub fn workspace_path(&self) -> &str {
        &self.state.event.workspace_path
    }

    /// 该实例的显式 workspace 身份（缺省时调用方按路径回退）。
    pub fn workspace_identity(&self) -> Option<&str> {
        self.state.event.workspace_identity.as_deref()
    }

    /// child 链路的子进程 pid（embedded 为 None）。
    pub fn process_id(&self) -> Option<u32> {
        self.state.event.process_id
    }

    /// 等本代结束并取结束记录（崩溃回收完成或收口完成后返回）。
    pub async fn exited(&self) -> ExitReport {
        let mut exit = self.state.exit.subscribe();
        loop {
            if let Some(report) = exit.borrow_and_update().clone() {
                return report;
            }
            // 发送端随实例状态存活；`self` 持有它，changed() 不会因关闭返回错误。
            let _ = exit.changed().await;
        }
    }

    /// 写入一行请求帧（只竞争写锁）。已是文本的入口（控制通道等）用它。
    pub async fn send_line(&self, line: &str) -> Result<()> {
        self.state.send_line(line).await
    }

    /// 写入一帧请求。类型化嵌入链路直接交 `Value`（阶段 4 免序列化）。
    pub async fn send_json(&self, value: &Value) -> Result<()> {
        self.state.send_json(value).await
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
            return Err(anyhow::Error::new(RuntimeClosed {
                method: method.to_owned(),
            }));
        }
        let mut envelope = json!({"id": id, "method": method, "params": params});
        if let Some(trace) = trace {
            envelope["trace"] = trace;
        }
        if let Err(error) = self.send_json(&envelope).await {
            self.state.lock_pending().remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(anyhow::Error::new(RuntimeClosed {
                method: method.to_owned(),
            })),
            Err(_) => {
                self.state.lock_pending().remove(&id);
                Err(anyhow::Error::new(RequestTimeout {
                    method: method.to_owned(),
                    timeout,
                }))
            }
        }
    }

    /// 请求收口并等待（宽限内）。先置 `stopping`：之后的读端关闭不算崩溃。
    /// 已被崩溃回收的实例再收口一次是幂等的（返回同一残余清单）。
    pub async fn shutdown(&self, grace: Duration) -> Vec<String> {
        self.state.stopping.store(true, Ordering::SeqCst);
        let mut lifecycle = self.state.link_lifecycle.lock().await;
        lifecycle.cancel();
        let (cause, residual) = match lifecycle.wait(grace).await {
            Ok(exit) => (exit.cause, exit.residual),
            Err(error) => (
                format!("{error:#}"),
                vec![format!("{}: {error}", self.identity)],
            ),
        };
        drop(lifecycle);
        self.state.record_exit(ExitReport {
            generation: self.generation,
            requested: true,
            cause,
            residual: residual.clone(),
        });
        residual
    }
}
