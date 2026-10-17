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

mod host_io;
mod instance;
pub mod link;
mod request_timeout;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use escode_cli_rust::embed::RuntimeOptions;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

pub use host_io::{HostError, HostReply, HostRequest, HostResponder, LifecycleEvent, RuntimeState};
pub use instance::{FrameSubscription, Instance};
pub use link::{Link, RuntimeLink};
pub use request_timeout::RequestTimeout;

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
    responder: host_io::ResponderSlot,
    lifecycle: Arc<host_io::LifecycleHub>,
}

impl Instances {
    pub fn new(link: Link) -> Self {
        Self {
            link: Arc::new(link),
            table: Arc::new(Mutex::new(Table::default())),
            grace: std::time::Duration::from_secs(5),
            responder: host_io::ResponderSlot::default(),
            lifecycle: Arc::default(),
        }
    }

    /// 反向请求应答器（组合根注入一次；未注入时一律回 `-32601`）。
    pub fn with_responder(self, responder: HostResponder) -> Self {
        self.set_responder(responder);
        self
    }

    /// 同上，但在实例表已共享后注入（组合根在 `rpcListen` 拿到 settings 根后才能装配应答器）；
    /// 已启动的实例在下一条反向请求上生效。
    pub fn set_responder(&self, responder: HostResponder) {
        self.responder.set(responder);
    }

    /// 订阅实例生命周期（每代 `available` / `unavailable` 各一次）。
    pub fn lifecycle(&self) -> mpsc::UnboundedReceiver<LifecycleEvent> {
        self.lifecycle.listen()
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
        let options = options();
        let workspace_path = options.cwd.to_string_lossy().into_owned();
        let workspace_identity = options.workspace_identity.clone();
        let runtime = match RuntimeLink::open(&self.link, options) {
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
        let process_id = runtime.lifecycle.process_id();
        let event = LifecycleEvent {
            identity: identity.to_owned(),
            workspace_path,
            workspace_identity,
            generation,
            runtime_identity: format!(
                "{identity}:{generation}:{}",
                process_id.map_or_else(|| "embedded".to_owned(), |pid| pid.to_string())
            ),
            process_id,
            state: RuntimeState::Available,
        };
        let instance = Instance::start(
            runtime,
            event,
            self.responder.clone(),
            Arc::clone(&self.lifecycle),
        );
        if let Some(slot) = self.lock().slots.get_mut(identity) {
            slot.instance = Some(Arc::clone(&instance));
        }
        // 登记完成才公告 available：监听方此时按 existing-only 一定取得到实例。
        instance.announce_available();
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

    /// 任意一个活实例（不启动）。`None` = 一个活的都没有。
    ///
    /// 供**不按 workspace 路由**的服务用（`usage-stats`：usage 表在全局 session 库，任何 client 回同一份
    /// 数据，TS 同取 `activeClientsByWorkspaceKey` 的第一个）。选中序 = 身份键字典序（`slots` 是 `BTreeMap`；
    /// TS 是连接序），该差异已登记在 `docs/specs/rust-u2-misc-channels.md` §1.3。
    pub fn live_any(&self) -> Option<Arc<Instance>> {
        let table = self.lock();
        table
            .slots
            .values()
            .filter_map(|slot| slot.instance.as_ref())
            .find(|instance| instance.is_alive())
            .map(Arc::clone)
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
