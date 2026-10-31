//! Source Server registry（S2–S4，docs/specs/source-server-remote-rust-host.md §9/§10/§11）。
//!
//! 唯一 owner：`sourceServerId`、Connection Profile、连接状态投影、显示名与 workspace 打开历史。
//! 持久化复用 app settings 的同名字段（`sourceServers` / `connectionProfiles` /
//! `sourceServerWorkspaceHistory`），不新增第二个持久化文件。
//!
//! S2 只读快照、S3/S4 连接与写面共用同一 owner：settings 是持久事实，运行态连接状态只在内存。
//! 写入经 [`writes`] 在 registry 写锁内落盘；连接状态由 [`crate::source_server_connections`]
//! 通过 [`SourceServerRegistry::set_connection_state`] 发布，避免第二个状态源。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc};

use crate::settings;

#[path = "source_server_identity.rs"]
mod identity;
#[path = "source_server_operation_log.rs"]
mod operation_log;
#[path = "source_server_settings_parse.rs"]
mod settings_parse;
#[path = "source_server_startup.rs"]
mod startup;
#[path = "source_server_types.rs"]
mod types;
#[path = "source_server_writes.rs"]
mod writes;

pub use identity::{IDENTITY_PROBE_TIMEOUT, ServerIdentity, probe_server_identity};
pub use operation_log::ConnectionOperationLog;
use settings_parse::parse_array;
use startup::LastOnlineWrite;
pub use startup::StartupReconnectCandidate;
pub use types::{
    ConnectionProfile, ConnectionStateSnapshot, DeclaredWorkspace, RunningTask, SourceServerRecord,
    WorkspaceHistoryEntry,
};
pub use writes::{
    DeclaredWorkspaceInput, HandshakeOutcome, HandshakeRecord, IdentityChangePending,
    UpdateProfileInput,
};

struct RegistryState {
    generation: u64,
    servers: Vec<SourceServerRecord>,
    profiles: Vec<ConnectionProfile>,
    workspace_history: Vec<WorkspaceHistoryEntry>,
    /// 操作日志 owner 在 S3；S2 保持空表但保留按 profile 的读取形状。
    logs: BTreeMap<String, Vec<Value>>,
    /// 运行态连接状态（S3）：连接管理器写入，覆盖 settings 派生的 S2 投影。
    connection_states: BTreeMap<String, ConnectionStateSnapshot>,
}

impl RegistryState {
    fn states(&self) -> Vec<ConnectionStateSnapshot> {
        self.profiles
            .iter()
            .map(|profile| self.state_for(profile))
            .collect()
    }

    fn state_for(&self, profile: &ConnectionProfile) -> ConnectionStateSnapshot {
        match self.connection_states.get(&profile.profile_id) {
            Some(state) => state.clone(),
            None => match &profile.last_connect_error {
                Some(error) => {
                    let mut state = ConnectionStateSnapshot::new(
                        &profile.profile_id,
                        profile.source_server_id.as_deref(),
                        "failed",
                        profile.last_connected_at.unwrap_or(0),
                    );
                    state.error = Some(error.clone());
                    state
                }
                None => ConnectionStateSnapshot::new(
                    &profile.profile_id,
                    profile.source_server_id.as_deref(),
                    "absent",
                    profile.last_connected_at.unwrap_or(0),
                ),
            },
        }
    }
}

/// registry 变更事件。S2 没有发布者（只读面），事件流保持不发射；
/// S3 的连接/写入流程成为唯一发布者，订阅端（Renderer）语义不变。
#[derive(Clone, Debug)]
pub enum SourceServerEvent {
    /// `onDidChangeState` 载荷（`sourceServerManagementStateDeltaSchema`）。
    State(Value),
    /// `onDidChangeDirectory` 载荷（`sourceServerManagementDirectoryDeltaSchema`）。
    Directory(u64),
    /// `onDidAppendOperationLog` 载荷（`sourceServerOperationLogSchema`）。
    OperationLog(Value),
}

/// 事件缓冲：订阅者短暂落后时丢弃最旧项，不阻塞发布者（与 Host 其它广播域一致）。
const EVENT_BUFFER: usize = 64;

/// Host 侧的 Source Server registry（每 Host 进程一份）。
pub struct SourceServerRegistry {
    state: Mutex<RegistryState>,
    /// 写面串行化：settings 落盘是异步的，必须在跨 await 期间挡住其它写命令，
    /// 否则同一份数组会在 read-modify-write 之间互相覆盖（spec §9.8）。
    write_lock: AsyncMutex<()>,
    /// 事件总线：`subscribe` 返回独立接收端；`Sender` 未持有接收端时发送会报错，
    /// 因此保留在 registry 上并只在 `subscribe` 中使用。
    events: broadcast::Sender<SourceServerEvent>,
    /// settings 装载失败时保留原因：`getSnapshot` 显式报错，不静默返回空快照（spec §9.4）。
    load_error: Option<String>,
    /// settings 根（`lastOnlineAt` 串行写队列用）；测试从对象构造时为空。
    home: Option<PathBuf>,
    /// 写队列发送端：连接管理器只入队，落盘由单消费者任务串行执行。
    last_online_tx: OnceLock<mpsc::UnboundedSender<LastOnlineWrite>>,
}

impl SourceServerRegistry {
    /// 从 settings 文件装载。只在 Host 启动装配时调用一次；S2 不做增量同步。
    ///
    /// 组合根是同步函数，这里用同步读（文件很小且只发生在启动），
    /// 读取语义与 `settings::read` 对齐：缺失 → 空；解析失败 → 显式错误。
    pub fn load(home: &Path) -> Self {
        let path = settings::settings_path(home);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::empty(),
            Err(error) => {
                return Self::failed(format!("read {}: {error}", path.display()));
            }
        };
        match serde_json::from_str::<Value>(&raw) {
            Ok(settings) => match Self::from_settings(&settings) {
                Ok(mut registry) => {
                    registry.home = Some(home.to_path_buf());
                    registry
                }
                Err(error) => Self::failed(format!("parse {}: {error:#}", path.display())),
            },
            Err(error) => Self::failed(format!("parse {}: {error}", path.display())),
        }
    }

    /// 空 registry：无 settings 时的合法视图（`generation` 从 1 开始）。
    pub fn empty() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                generation: 1,
                servers: Vec::new(),
                profiles: Vec::new(),
                workspace_history: Vec::new(),
                logs: BTreeMap::new(),
                connection_states: BTreeMap::new(),
            }),
            write_lock: AsyncMutex::new(()),
            events: broadcast::channel(EVENT_BUFFER).0,
            load_error: None,
            home: None,
            last_online_tx: OnceLock::new(),
        }
    }

    /// 从 settings 对象构造（测试与 [`Self::load`] 共用）。
    pub fn from_settings(settings: &Value) -> Result<Self> {
        let servers = parse_array::<SourceServerRecord>(settings, "sourceServers")?;
        let profiles = parse_array::<ConnectionProfile>(settings, "connectionProfiles")?;
        let workspace_history =
            parse_array::<WorkspaceHistoryEntry>(settings, "sourceServerWorkspaceHistory")?;
        Ok(Self {
            state: Mutex::new(RegistryState {
                generation: 1,
                servers,
                profiles,
                workspace_history,
                logs: BTreeMap::new(),
                connection_states: BTreeMap::new(),
            }),
            write_lock: AsyncMutex::new(()),
            events: broadcast::channel(EVENT_BUFFER).0,
            load_error: None,
            home: None,
            last_online_tx: OnceLock::new(),
        })
    }

    fn failed(error: String) -> Self {
        let mut registry = Self::empty();
        registry.load_error = Some(error);
        registry
    }

    fn lock(&self) -> MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 订阅 registry 变更（S2 无发布者，流不发射）。
    pub fn subscribe(&self) -> broadcast::Receiver<SourceServerEvent> {
        self.events.subscribe()
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// 目录内容变化（含迁移统计补全/失效）：递增 generation 并广播 `onDidChangeDirectory`。
    ///
    /// 只发布事件，不改写 registry 数据；统计 owner 在读取完成后调用。
    pub fn publish_directory_change(&self) {
        let generation = {
            let mut registry = self.lock();
            registry.generation += 1;
            registry.generation
        };
        let _ = self.events.send(SourceServerEvent::Directory(generation));
    }

    /// `source-server-management.getSnapshot`：锁内复制，不做 IO。
    pub fn snapshot(&self) -> Result<Value, String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        let state = self.lock();
        Ok(json!({
            "generation": state.generation,
            "profiles": &state.profiles,
            "servers": &state.servers,
            "workspaceHistory": &state.workspace_history,
            "states": state.states(),
            "failedPathCount": 0,
            "abandonedPathCount": 0,
            "migrationCounts": [],
        }))
    }

    /// 按 profileId 取 profile（连接管理器解析 transport 用）。
    pub fn profile(&self, profile_id: &str) -> Option<ConnectionProfile> {
        self.lock()
            .profiles
            .iter()
            .find(|profile| profile.profile_id == profile_id)
            .cloned()
    }

    /// 当前投影状态（运行态优先，其次 settings 派生的 `absent` / `failed`）。
    pub fn connection_state(&self, profile_id: &str) -> Option<ConnectionStateSnapshot> {
        let state = self.lock();
        state
            .profiles
            .iter()
            .find(|profile| profile.profile_id == profile_id)
            .map(|profile| state.state_for(profile))
    }

    /// Source 记录（declaredWorkspaces 等），用于更新前收敛运行任务范围。
    pub fn source_server(&self, source_server_id: &str) -> Option<SourceServerRecord> {
        self.lock()
            .servers
            .iter()
            .find(|server| server.source_server_id == source_server_id)
            .cloned()
    }

    /// 属于同一 Source 的所有 profile（`disconnectSource` 的作用域）。
    pub fn profile_ids_for_source(&self, source_server_id: &str) -> Vec<String> {
        self.lock()
            .profiles
            .iter()
            .filter(|profile| profile.source_server_id.as_deref() == Some(source_server_id))
            .map(|profile| profile.profile_id.clone())
            .collect()
    }

    /// 写入运行态连接状态：递增 generation 并广播 `onDidChangeState` 载荷。
    ///
    /// 连接管理器是唯一调用方；这里不持久化（重启用 settings 投影，见 spec §10.10）。
    pub fn set_connection_state(&self, state: ConnectionStateSnapshot) {
        let previous = {
            let registry = self.lock();
            registry
                .connection_states
                .get(&state.profile_id)
                .map(|previous| previous.state)
        };
        let record_last_online = (previous == Some("online") && state.state != "online")
            || (previous != Some("online") && state.state == "online");
        let generation = {
            let mut registry = self.lock();
            registry.generation += 1;
            registry
                .connection_states
                .insert(state.profile_id.clone(), state.clone());
            registry.generation
        };
        let _ = self.events.send(SourceServerEvent::State(json!({
            "generation": generation,
            "state": state,
        })));
        if record_last_online && let Some(source_server_id) = state.source_server_id.as_deref() {
            self.record_last_online_now(source_server_id, state.updated_at);
        }
    }

    /// `source-server-management.getRecentOperationLogs`：未知 profile 返回空数组。
    pub fn recent_operation_logs(&self, profile_id: &str) -> Vec<Value> {
        self.lock()
            .logs
            .get(profile_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 追加一条操作日志并广播 `onDidAppendOperationLog`。
    ///
    /// 只为保持 UI 可读保留最近 200 条；日志不是审计事实，超出后丢弃最旧项。
    pub fn append_operation_log(&self, log: ConnectionOperationLog) {
        const MAX_LOGS_PER_PROFILE: usize = 200;
        let Ok(value) = serde_json::to_value(&log) else {
            return;
        };
        {
            let mut registry = self.lock();
            let logs = registry.logs.entry(log.profile_id).or_default();
            logs.push(value.clone());
            if logs.len() > MAX_LOGS_PER_PROFILE {
                logs.remove(0);
            }
        }
        let _ = self.events.send(SourceServerEvent::OperationLog(value));
    }
}

#[cfg(test)]
#[path = "source_server_tests.rs"]
mod tests;
