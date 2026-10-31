//! Source Server 连接管理器（S3-a，docs/specs/source-server-remote-rust-host.md §10.10）。
//!
//! 唯一 owner：profileId → 活动 [`RemoteConnection`] 与 per-profile 尝试代次。
//! 连接状态不在这里另存：状态写入 [`SourceServerRegistry`]，由 registry 递增 generation 并
//! 广播 `onDidChangeState`，保证快照与事件同源。
//!
//! transport 启动命令由调用方注入（生产 = `source_server_transport`，测试 = fixture），
//! 与 `Instances::with_link_for` 同形：链路差异不进入本模块。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::watch;

use crate::source_server::{
    ConnectionOperationLog, ConnectionProfile, ConnectionStateSnapshot, SourceServerRegistry,
};
use crate::source_server_bootstrap::InstallOptions;
use crate::source_server_client::RemoteChannelClient;
use crate::source_server_connection::RemoteConnection;
use crate::source_server_connection_projection::online_state;
use crate::source_server_connection_state::{StateWriter, now_ms};
use crate::source_server_exec::ResolvedTransport;
use crate::source_server_migration_counts::SourceServerMigrationCounts;
use crate::source_server_update::{UpdateError, UpdateRequest};

#[path = "source_server_connections_migration.rs"]
mod migration;

#[path = "source_server_connections_establish.rs"]
mod establish;

#[path = "source_server_connections_provisioning.rs"]
mod provisioning;

#[path = "source_server_connections_cancel.rs"]
mod cancel;

#[path = "source_server_connection_options.rs"]
mod options;

pub use cancel::ConnectionCanceled;
pub use options::{ConnectIntent, ConnectionsOptions};

/// 握手 + Initialize 等待上限。只用于「远端没有响应」的显式失败，不作为同步手段。
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);

pub const DEFAULT_CLIENT_ID: &str = "escode-desktop-host";

/// transport 解析器的异步结果：凭据解密（`credential` 域）本身是异步 IO。
pub type TransportFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<ResolvedTransport>> + Send>>;

/// transport 解析器：profile → 启动命令（失败即显式失败，不回退其它 transport）。
pub type TransportResolver = Arc<dyn Fn(&ConnectionProfile) -> TransportFuture + Send + Sync>;

#[derive(Default)]
struct Slot {
    /// per-profile 尝试代次：过期结果不得覆盖新一代状态（spec §10.5 / §10.10）。
    attempt: u64,
    /// 单飞：同一 profile 的并发 connect 只允许一个走启动路径。
    starting: Arc<AsyncMutex<()>>,
    connection: Option<RemoteConnection>,
}

type Slots = Mutex<BTreeMap<String, Slot>>;

fn lock_slots(slots: &Slots) -> MutexGuard<'_, BTreeMap<String, Slot>> {
    slots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Host 进程内唯一的 Source Server 连接表。
pub struct SourceServerConnections {
    registry: Arc<SourceServerRegistry>,
    options: ConnectionsOptions,
    slots: Arc<Slots>,
    /// 迁移统计投影（S4-f）：只活在 Host 进程内，不持久化。
    migration_counts: Arc<SourceServerMigrationCounts>,
    /// 诊断/测试计数：成功建立的连接次数（不参与状态）。
    connected: AtomicU64,
    /// 首连取消登记（S5-a cancel）：caller key → 取消信号，不持有连接事实。
    cancellations: Mutex<BTreeMap<String, watch::Sender<bool>>>,
    /// 自动恢复任务需要 `Arc<Self>`；组合根创建 Arc 后登记弱引用（测试可跳过）。
    self_weak: OnceLock<Weak<Self>>,
}

impl SourceServerConnections {
    pub fn new(registry: Arc<SourceServerRegistry>, options: ConnectionsOptions) -> Self {
        let migration_counts = Arc::new(SourceServerMigrationCounts::new(Arc::clone(&registry)));
        Self {
            registry,
            options,
            slots: Arc::new(Mutex::new(BTreeMap::new())),
            migration_counts,
            cancellations: Mutex::new(BTreeMap::new()),
            connected: AtomicU64::new(0),
            self_weak: OnceLock::new(),
        }
    }

    /// 登记弱引用，使 `watch_close` 能启动自动恢复；无此登记的测试保持立即 offline。
    pub fn install_self(self: &Arc<Self>) {
        let _ = self.self_weak.set(Arc::downgrade(self));
    }

    fn recovery_handle(&self) -> Option<Arc<Self>> {
        self.self_weak.get().and_then(Weak::upgrade)
    }

    pub fn registry(&self) -> &Arc<SourceServerRegistry> {
        &self.registry
    }

    pub(crate) fn options(&self) -> &ConnectionsOptions {
        &self.options
    }

    /// 更新换代：先递增尝试代次让旧 watcher 失效，再交出旧连接。
    pub(crate) fn begin_update_swap(&self, profile_id: &str) -> (u64, Option<RemoteConnection>) {
        let mut slots = self.lock();
        let slot = slots.entry(profile_id.to_owned()).or_default();
        slot.attempt += 1;
        (slot.attempt, slot.connection.take())
    }

    pub(crate) fn record_connected(&self) {
        self.connected.fetch_add(1, Ordering::Relaxed);
    }

    /// 已成功建立的连接次数（诊断/测试）。
    pub fn connected_count(&self) -> u64 {
        self.connected.load(Ordering::Relaxed)
    }

    /// 建立（或复用）一条 profile 连接。失败时已写入 `failed` 状态并返回错误。
    pub async fn connect(&self, profile_id: &str) -> Result<()> {
        self.connect_with(profile_id, ConnectIntent::Explicit).await
    }

    /// 用户确认身份替换后重连：只有该意图允许 retire 旧 Profile 并重绑定新身份。
    pub async fn connect_confirming_identity(&self, profile_id: &str) -> Result<()> {
        self.connect_with(profile_id, ConnectIntent::ConfirmIdentityChange)
            .await
    }

    /// 带连接意图的入口：`Recover` 不安装新版本（§11.5），缺 release 时停在 `needs-install`。
    pub async fn connect_with(&self, profile_id: &str, intent: ConnectIntent) -> Result<()> {
        let profile = self
            .registry
            .profile(profile_id)
            .with_context(|| format!("unknown source server profile: {profile_id}"))?;
        let starting = self.starting_lock(profile_id);
        let _guard = starting.lock_owned().await;

        if self.live_client(profile_id).is_some() {
            return Ok(());
        }

        let previous = {
            let mut slots = self.lock();
            let slot = slots.entry(profile_id.to_owned()).or_default();
            slot.attempt += 1;
            slot.connection.take()
        };
        if let Some(previous) = previous {
            let _ = previous.shutdown().await;
        }
        let attempt = self.current_attempt(profile_id);
        self.state().publish(
            profile.source_server_id.as_deref(),
            profile_id,
            "bootstrapping",
            None,
            None,
            None,
            None,
        );
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                profile_id,
                "connect",
                "connecting",
                "info",
                format!(
                    "connecting to source server profile {}",
                    profile.display_name.as_deref().unwrap_or(profile_id)
                ),
                now_ms(),
            ));

        let establish::Established {
            connection,
            closed,
            version,
            compatibility,
            effective,
        } = self.establish(&profile, intent).await?;
        let effective_id = effective.profile_id.clone();
        let effective_attempt = if effective_id == profile_id {
            attempt
        } else {
            if self.current_attempt(profile_id) != attempt {
                let _ = connection.shutdown().await;
                return Ok(());
            }
            // 身份替换后旧 Profile 已 retired，不能再停留在 bootstrapping/online（spec §11.12）。
            self.registry
                .set_connection_state(ConnectionStateSnapshot::new(
                    profile_id,
                    profile.source_server_id.as_deref(),
                    "offline",
                    now_ms(),
                ));
            self.next_attempt(&effective_id)
        };
        if let Some(home) = self.options.preferences_home.clone() {
            crate::source_server_runtime_preferences::attach(&connection.client, home);
        }
        if let Err(error) = self
            .provision_before_online(
                &effective,
                &effective_id,
                effective_attempt,
                &connection.client,
                intent,
            )
            .await
        {
            let _ = connection.shutdown().await;
            return Err(error);
        }
        if !self.store_if_current(&effective_id, effective_attempt, connection) {
            return Ok(());
        }
        self.connected.fetch_add(1, Ordering::Relaxed);
        let state = online_state(
            &effective,
            &version,
            compatibility.as_ref(),
            &self.options.compatibility,
            now_ms(),
        );
        self.registry.set_connection_state(state);
        self.registry
            .append_operation_log(ConnectionOperationLog::new(
                &effective_id,
                "connect",
                "online",
                "info",
                format!("source server {version} is online"),
                now_ms(),
            ));
        self.watch_close(effective_id, effective_attempt, closed);
        Ok(())
    }

    /// 显式更新（spec §11.4）：编排在 `source_server_update_flow`，本类型只提供连接原语。
    pub async fn update_source(
        &self,
        request: UpdateRequest,
    ) -> std::result::Result<(), UpdateError> {
        crate::source_server_update_flow::update_source(self, request).await
    }

    /// 关闭该 Source 的所有活动 profile，并 await 子进程退出。无活动连接时幂等成功。
    pub async fn disconnect_source(&self, source_server_id: &str) {
        for profile_id in self.registry.profile_ids_for_source(source_server_id) {
            self.disconnect(&profile_id).await;
        }
    }

    /// 关闭一条 profile 连接（显式收口，不自动重连）。
    pub async fn disconnect(&self, profile_id: &str) {
        let Some(profile) = self.registry.profile(profile_id) else {
            return;
        };
        let starting = self.starting_lock(profile_id);
        let _guard = starting.lock_owned().await;
        let connection = {
            let mut slots = self.lock();
            let slot = slots.entry(profile_id.to_owned()).or_default();
            // 先递增代次：收口期间 watcher 的 `offline` 属于过期尝试，不得覆盖 `absent`。
            slot.attempt += 1;
            slot.connection.take()
        };
        if let Some(provisioning) = self.options.provisioning.as_ref() {
            provisioning.remove(profile_id);
        }
        if let Some(connection) = connection
            && let Err(error) = connection.shutdown().await
        {
            escode_host_control::logging::warn(
                "source-server",
                format!("shutdown {profile_id}: {error:#}"),
            );
        }
        self.state().publish(
            profile.source_server_id.as_deref(),
            profile_id,
            "absent",
            None,
            None,
            None,
            None,
        );
    }

    /// 已初始化的客户端（后续目录绑定 / scoped 转发从这里取）。
    pub fn client(&self, profile_id: &str) -> Option<Arc<RemoteChannelClient>> {
        self.live_client(profile_id)
    }

    /// 某个 Source 的在线客户端：多 Profile 时取第一条已握手完成的连接。
    ///
    /// `ensureConversationWorkspace` 与迁移报错都要求“该设备在线”，但不需要指定具体 Profile。
    pub fn online_client(&self, source_server_id: &str) -> Option<Arc<RemoteChannelClient>> {
        self.registry
            .profile_ids_for_source(source_server_id)
            .into_iter()
            .find_map(|profile_id| self.live_client(&profile_id))
    }

    /// 活动连接的 hello（版本 / 远端 pid）；未连接时为 `None`。
    pub fn hello(&self, profile_id: &str) -> Option<crate::source_server_handshake::Hello> {
        self.lock()
            .get(profile_id)
            .and_then(|slot| slot.connection.as_ref())
            .map(|connection| connection.hello.clone())
    }

    pub(crate) fn live_client(&self, profile_id: &str) -> Option<Arc<RemoteChannelClient>> {
        self.lock()
            .get(profile_id)
            .and_then(|slot| slot.connection.as_ref())
            .map(|connection| Arc::clone(&connection.client))
    }

    pub(crate) fn starting_lock(&self, profile_id: &str) -> Arc<AsyncMutex<()>> {
        self.lock()
            .entry(profile_id.to_owned())
            .or_default()
            .starting
            .clone()
    }

    pub(crate) fn current_attempt(&self, profile_id: &str) -> u64 {
        self.lock().get(profile_id).map_or(0, |slot| slot.attempt)
    }

    /// 身份替换后为新 Profile 分配独立尝试代次。
    pub(crate) fn next_attempt(&self, profile_id: &str) -> u64 {
        let mut slots = self.lock();
        let slot = slots.entry(profile_id.to_owned()).or_default();
        slot.attempt += 1;
        slot.attempt
    }

    /// 登记连接；若尝试已被收口（代次变化）则收口这份连接并返回 false。
    pub(crate) fn store_if_current(
        &self,
        profile_id: &str,
        attempt: u64,
        connection: RemoteConnection,
    ) -> bool {
        let stale = {
            let mut slots = self.lock();
            let slot = slots.entry(profile_id.to_owned()).or_default();
            if slot.attempt == attempt {
                slot.connection = Some(connection);
                None
            } else {
                Some(connection)
            }
        };
        // 过期连接在锁外收口：不在持锁期间 await。
        if let Some(connection) = stale {
            tokio::spawn(async move {
                let _ = connection.shutdown().await;
            });
            return false;
        }
        true
    }

    /// 链路关闭 watcher：只有 attempt 仍是当前值时才把该 profile 改判 `offline`。
    pub(crate) fn watch_close(
        &self,
        profile_id: String,
        attempt: u64,
        mut closed: watch::Receiver<bool>,
    ) {
        let registry = Arc::clone(&self.registry);
        let slots = Arc::clone(&self.slots);
        let manager = self.recovery_handle();
        tokio::spawn(async move {
            // sender 随连接对象 drop 也会让 `changed()` 返回 Err：两种情况都视为链路结束。
            let _ = closed.changed().await;
            let connection = {
                let mut slots = lock_slots(&slots);
                match slots.get_mut(&profile_id) {
                    Some(slot) if slot.attempt == attempt => slot.connection.take(),
                    _ => None,
                }
            };
            let Some(connection) = connection else {
                return;
            };
            let previous = registry.connection_state(&profile_id);
            if let (Some(manager), Some(previous)) = (manager, previous) {
                registry.set_connection_state(
                    crate::source_server_connection_state::retrying_state(
                        &previous,
                        Some("remote transport closed".to_owned()),
                        1,
                        crate::source_server_connection_state::RECOVERY_LIMIT,
                    ),
                );
                let _ = connection.shutdown().await;
                tokio::spawn(manager.run_recovery(profile_id, attempt));
                return;
            }
            let source_server_id = registry
                .profile(&profile_id)
                .and_then(|profile| profile.source_server_id);
            let mut state = ConnectionStateSnapshot::new(
                &profile_id,
                source_server_id.as_deref(),
                "offline",
                now_ms(),
            );
            state.error = Some("remote transport closed".to_owned());
            state.retryable = Some(true);
            registry.set_connection_state(state);
            let _ = connection.shutdown().await;
        });
    }

    fn state(&self) -> StateWriter<'_> {
        StateWriter {
            registry: &self.registry,
            compatibility: &self.options.compatibility,
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Slot>> {
        lock_slots(&self.slots)
    }
}

#[cfg(test)]
#[path = "source_server_connections_tests.rs"]
mod tests;
