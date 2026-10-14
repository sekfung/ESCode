//! 运行时实例表（rust-unified-host.md §2 生命周期；rust-u2-host-skeleton.md §3.1/§6）。
//!
//! ```text
//! get(ws) --[槽位 single-flight]--> spawn gen N+1 --> available
//!    ^                                                    |
//!    |  崩溃/退出 --> unavailable --（懒）下次 get 才 spawn gen N+2
//!    |                                                    |
//!    +- cancel --> drain（宽限）--> 残余清单 + 世代记账
//! ```
//!
//! Host 持有实例表、代次与调度（AGENTS.md 状态所有者）；runtime 实例只归属一个
//! workspace 身份（`workspaceIdentity ?? workspacePath`，AGENTS.md Workspace Identity）。

pub mod link;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use escode_cli_rust::embed::RuntimeOptions;
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

pub use link::{Link, RuntimeLink};

/// 一个 workspace 的实例槽：世代号 + 当前实例。
struct Slot {
    generation: u64,
    instance: Option<Arc<Instance>>,
    /// 单飞：同一 workspace 的并发 `get` 只允许一个走启动路径。
    starting: Arc<AsyncMutex<()>>,
}

#[derive(Default)]
struct Table {
    slots: BTreeMap<String, Slot>,
}

/// 实例表的共享句柄（Host 内唯一；每个 workspace 一个实例）。
#[derive(Clone)]
pub struct Instances {
    link: Arc<Link>,
    table: Arc<Mutex<Table>>,
    grace: std::time::Duration,
}

/// 一个已启动的运行时实例：链路 + 世代 + 收口句柄。
/// 读端与写端各持一把锁（帧泵长期持读锁，写请求不能被它阻塞）；
/// 生命周期（存活/收口）在第三把同步锁下，与读写互不等待。
pub struct Instance {
    pub identity: String,
    pub generation: u64,
    /// 读端由实例内的读循环独占（`reader`）：出站帧按 id 关联到 `call` 的等待方，
    /// 其余帧扇出给全部订阅者（Host 内的帧泵）。这是 Host 唯一读该实例的地方。
    reader: tokio::sync::Mutex<Option<link::RuntimeReader>>,
    writer: tokio::sync::Mutex<link::RuntimeWriter>,
    lifecycle: tokio::sync::Mutex<link::Lifecycle>,
    /// 读循环与请求方共享的状态（读循环持 Arc，不需要自引用）。
    state: Arc<InstanceState>,
}

impl InstanceState {
    fn lock_pending(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<String, tokio::sync::oneshot::Sender<Value>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_subscribers(&self) -> std::sync::MutexGuard<'_, Vec<mpsc::Sender<String>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 实例出站流的分发状态：在飞请求 + 帧订阅者 + 请求序号。
#[derive(Default)]
struct InstanceState {
    /// 在飞请求：`id` → 应答等待方。
    pending: std::sync::Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<Value>>>,
    /// 帧订阅者（每个 attachment 一个）；读循环向它们扇出非应答帧。
    subscribers: std::sync::Mutex<Vec<mpsc::Sender<String>>>,
    /// 请求序号（生成关联 id）。
    requests: std::sync::atomic::AtomicU64,
    /// 运行时是否已报到（`startup/storageState` phase=ready）；读循环置位。
    ready: std::sync::atomic::AtomicBool,
    /// 读循环是否已启动。
    pumping: std::sync::atomic::AtomicBool,
}

impl Instances {
    pub fn new(link: Link) -> Self {
        Self {
            link: Arc::new(link),
            table: Arc::new(Mutex::new(Table::default())),
            grace: std::time::Duration::from_secs(5),
        }
    }

    /// 关闭宽限（wind-down 预算：工具进程树 TERM→KILL 收口上限）。
    pub fn with_grace(mut self, grace: std::time::Duration) -> Self {
        self.grace = grace;
        self
    }

    /// 取用一个 workspace 的实例：活着就复用；已退出则**懒重启**下一代
    /// （崩溃/退出 → unavailable → 下次 get 才 spawn）。
    pub async fn get(
        &self,
        identity: &str,
        options: impl FnOnce() -> RuntimeOptions,
    ) -> Result<Arc<Instance>> {
        if let Some(instance) = self.live(identity) {
            return Ok(instance);
        }
        let starting = self.starting_lock(identity);
        let _guard = starting.lock().await;
        // 拿到槽位锁后再查一次：并发请求里只有一个真正启动。
        if let Some(instance) = self.live(identity) {
            return Ok(instance);
        }
        let (generation, previous) = {
            let mut table = self.lock();
            let slot = table
                .slots
                .entry(identity.to_owned())
                .or_insert_with(|| Slot {
                    generation: 0,
                    instance: None,
                    starting: Arc::new(AsyncMutex::new(())),
                });
            slot.generation += 1;
            (slot.generation, slot.instance.take())
        };
        // 旧实例已退出（否则 live() 会命中）；这里兜底收口一次，确保工具进程树回收。
        if let Some(previous) = previous {
            let _ = previous.shutdown(self.grace).await;
        }
        let runtime = match RuntimeLink::open(&self.link, options()) {
            Ok(runtime) => runtime,
            Err(error) => {
                // 启动失败不占世代名额：回退计数，下一次 get 仍是同一代次语义。
                let mut table = self.lock();
                if let Some(slot) = table.slots.get_mut(identity)
                    && slot.instance.is_none()
                {
                    slot.generation = generation - 1;
                }
                return Err(error);
            }
        };
        let link::RuntimeLink {
            reader,
            writer,
            lifecycle,
        } = runtime;
        let instance = Arc::new(Instance {
            identity: identity.to_owned(),
            generation,
            reader: tokio::sync::Mutex::new(Some(reader)),
            writer: tokio::sync::Mutex::new(writer),
            lifecycle: tokio::sync::Mutex::new(lifecycle),
            state: Arc::new(InstanceState {
                requests: std::sync::atomic::AtomicU64::new(1),
                ..Default::default()
            }),
        });
        if let Some(slot) = self.lock().slots.get_mut(identity) {
            slot.instance = Some(Arc::clone(&instance));
        }
        Ok(instance)
    }

    /// 已有的活实例（不启动）。`None` = 没有实例或实例已退出。
    pub fn live(&self, identity: &str) -> Option<Arc<Instance>> {
        let table = self.lock();
        let slot = table.slots.get(identity)?;
        let instance = slot.instance.as_ref()?;
        if instance.is_alive() {
            Some(Arc::clone(instance))
        } else {
            None
        }
    }

    /// 当前 generations 快照（诊断/测试）。
    pub fn generations(&self) -> BTreeMap<String, u64> {
        self.lock()
            .slots
            .iter()
            .map(|(key, slot)| (key.clone(), slot.generation))
            .collect()
    }

    pub fn identities(&self) -> Vec<String> {
        self.lock().slots.keys().cloned().collect()
    }

    /// 关闭一个 workspace 的实例：cancel → drain（宽限）→ 记录残余；槽位保留世代。
    pub async fn close(&self, identity: &str) -> Result<Vec<String>> {
        let instance = {
            let mut table = self.lock();
            match table.slots.get_mut(identity) {
                Some(slot) => slot.instance.take(),
                None => None,
            }
        };
        let Some(instance) = instance else {
            return Ok(Vec::new());
        };
        let residual = instance.shutdown(self.grace).await;
        Ok(residual)
    }

    /// 全部收口（Host 退出路径）：逐个 close，返回全部残余任务/进程。
    pub async fn shutdown(&self) -> Vec<String> {
        let identities = self.identities();
        let mut residual = Vec::new();
        for identity in identities {
            match self.close(&identity).await {
                Ok(names) => residual.extend(names),
                Err(error) => residual.push(format!("{identity}: {error}")),
            }
        }
        residual
    }

    fn starting_lock(&self, identity: &str) -> Arc<AsyncMutex<()>> {
        let mut table = self.lock();
        Arc::clone(
            &table
                .slots
                .entry(identity.to_owned())
                .or_insert_with(|| Slot {
                    generation: 0,
                    instance: None,
                    starting: Arc::new(AsyncMutex::new(())),
                })
                .starting,
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Instance {
    /// 实例是否仍在运行。同步判定：生命周期锁在收口/等待期间被持有，
    /// 此时按存活处理（真正的关闭会在下一次读写上暴露）。
    pub fn is_alive(&self) -> bool {
        match self.lifecycle.try_lock() {
            Ok(lifecycle) => lifecycle.is_alive(),
            Err(_) => true,
        }
    }

    /// 写入一行请求帧（只竞争写锁）。
    pub async fn send_line(&self, line: &str) -> Result<()> {
        self.writer.lock().await.send_line(line).await
    }

    /// 订阅出站帧（每个 attachment 一个）；读循环负责扇出。
    pub async fn subscribe(&self) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(256);
        self.state.lock_subscribers().push(tx);
        self.start_reader().await;
        rx
    }

    /// 等运行时报到（`startup/storageState` phase=ready）；实例退出即提前返回 false。
    /// 就绪标志由读循环置位，因此这里先启动读循环（幂等）。
    pub async fn wait_ready(&self, timeout: Duration) -> bool {
        self.start_reader().await;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.state.ready.load(std::sync::atomic::Ordering::SeqCst) {
                return true;
            }
            if !self.is_alive() {
                return false;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 向 runtime 发一条请求并等应答（按 id 关联；`timeout` 到点即失败）。
    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = format!(
            "host-{}",
            self.state
                .requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.state.lock_pending().insert(id.clone(), tx);
        self.start_reader().await;
        let line = serde_json::json!({"id": id, "method": method, "params": params}).to_string();
        if let Err(error) = self.send_line(&line).await {
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

    /// 启动读循环（幂等）：一次读端、按 id 关联应答、其余帧扇出给订阅者。
    /// 读循环持有分发状态（`Arc<InstanceState>`），不依赖实例自身。
    async fn start_reader(&self) {
        if self
            .state
            .pumping
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let mut reader = match self.reader.lock().await.take() {
            Some(reader) => reader,
            None => return,
        };
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            while let Some(line) = reader.line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed = serde_json::from_str::<Value>(trimmed).ok();
                if parsed.as_ref().is_some_and(|value| {
                    value["method"] == "startup/storageState" && value["params"]["phase"] == "ready"
                }) {
                    state.ready.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let routed = parsed
                    .as_ref()
                    .and_then(|value| value["id"].as_str().map(str::to_owned))
                    .and_then(|id| state.lock_pending().remove(&id))
                    .is_some_and(|waiter| waiter.send(parsed.unwrap_or(Value::Null)).is_ok());
                if routed {
                    continue;
                }
                let targets = state.lock_subscribers().clone();
                for target in targets {
                    let _ = target.try_send(trimmed.to_owned());
                }
            }
        });
    }

    /// 请求收口并等待（宽限内）。
    pub async fn shutdown(&self, grace: std::time::Duration) -> Vec<String> {
        let mut lifecycle = self.lifecycle.lock().await;
        lifecycle.cancel();
        match lifecycle.wait(grace).await {
            Ok(residual) => residual,
            Err(error) => vec![format!("{}: {error}", self.identity)],
        }
    }
}
