//! 每条 RPC 连接一个的 V4 作用域（TS `escodeAgentConnectionScope.ts` 的
//! `createESCodeAgentConnectionScope`；角色固定为 terminal-client——Rust Host 不做 trusted relay）。
//!
//! 只持连接级「谁拥有哪个订阅」的路由事实：不缓存帧内容、不改帧（docs/specs/rust-u2-agent-v4.md §2）。
//!
//! 保序：帧路由（判定归属/暂存/投递）与「ACK 后登记 owner 并按到达序释放暂存帧」在同一把锁下完成，
//! 所以登记之后到达的实时帧不会插到暂存帧之前（TS 单线程天然满足，Rust 需显式保证）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::base::{Base, Connection};
use super::frames::{RoutedFrame, TopicKind};
use crate::forward::RuntimePolicy;
use crate::session::{ClientMode, FlowState, RpcError};

/// ACK 窗口暂存上限（TS `MAX_PENDING_OWNERSHIP_FRAMES` / `_BYTES`）：溢出整批清空并显式失败，
/// 不能 shift 最旧帧（会把一个逻辑帧变成永久缺片）。
pub(super) const MAX_PENDING_FRAMES: usize = 1_024;
pub(super) const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Owned {
    pub kind: TopicKind,
    pub ownership_key: String,
    pub subscription_id: String,
    pub topic: String,
    pub workspace_key: String,
    /// TS `workspaceTarget(params)`：`{workspacePath, workspaceIdentity?}`。
    pub target: Value,
}

pub(super) struct Pending {
    pub(super) kind: TopicKind,
    pub(super) group_key: String,
    pub(super) workspace_key: String,
    pub(super) frames: Vec<Arc<RoutedFrame>>,
    pub(super) staged_bytes: usize,
    pub(super) overflow: Option<&'static str>,
    pub(super) generation: u64,
    pub(super) invalidated: bool,
}

pub(super) struct Route {
    pub(super) senders: Vec<mpsc::UnboundedSender<Value>>,
    pub(super) task: JoinHandle<()>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) disposed: bool,
    pub(super) handshake_complete: bool,
    pub(super) hello_issued: bool,
    pub(super) bound_client_id: Option<String>,
    pub(super) workflow_run_deltas: bool,
    pub(super) command_query_workspace: Option<String>,
    pub(super) owned: BTreeMap<String, Owned>,
    pub(super) by_ownership: BTreeMap<String, String>,
    pub(super) pending: BTreeMap<u64, Pending>,
    pub(super) next_pending: u64,
    pub(super) generations: BTreeMap<String, u64>,
    pub(super) forwarded_flow: BTreeMap<String, FlowState>,
    /// attachment 曾触及的 workspace route（无订阅也要能 trusted closed）：flow key → target。
    pub(super) attachment_routes: BTreeMap<String, Value>,
    pub(super) routes: BTreeMap<(TopicKind, String), Route>,
    pub(super) current_flow: Option<FlowState>,
    pub(super) flow_closed: bool,
}

/// 一条连接的 V4 作用域。
pub(crate) struct Scope {
    pub(super) base: Arc<Base>,
    pub(super) connection_id: String,
    pub(super) client_mode: ClientMode,
    pub(super) state: Mutex<State>,
    /// TS `flowUpdateChain`：水位边沿与 close 按提交序串行转发。
    pub(super) flow_chain: tokio::sync::Mutex<()>,
    pub(super) tasks: Mutex<Vec<JoinHandle<()>>>,
}

pub(super) fn fault(message: &str) -> RpcError {
    RpcError::new("Error", message)
}

/// 身份键：`workspaceIdentity?.trim() || workspacePath`。
pub(super) fn workspace_key(params: &Value) -> String {
    params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            params["workspacePath"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
}

/// TS `workspaceTarget`：只保留 workspacePath 与（真值时的）workspaceIdentity。
pub(super) fn workspace_target(params: &Value) -> Value {
    let mut target = json!({"workspacePath": params["workspacePath"]});
    if let Some(identity) = params["workspaceIdentity"]
        .as_str()
        .filter(|v| !v.is_empty())
    {
        target["workspaceIdentity"] = Value::from(identity);
    }
    target
}

pub(super) fn flow_key(workspace_key: &str, connection_id: &str) -> String {
    format!("{workspace_key}\0{connection_id}")
}

impl Scope {
    pub(crate) fn new(
        base: Arc<Base>,
        connection_id: String,
        client_mode: ClientMode,
    ) -> Arc<Self> {
        Arc::new(Self {
            base,
            connection_id,
            client_mode,
            state: Mutex::default(),
            flow_chain: tokio::sync::Mutex::new(()),
            tasks: Mutex::default(),
        })
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn connection(&self) -> Connection {
        Connection {
            connection_id: self.connection_id.clone(),
            client_mode: self.client_mode,
            workflow_run_deltas: self.lock().workflow_run_deltas,
        }
    }

    pub(super) fn assert_open(&self) -> Result<(), RpcError> {
        if self.lock().disposed {
            return Err(fault("fault.connection.closed"));
        }
        Ok(())
    }

    pub(super) fn assert_ready(&self) -> Result<(), RpcError> {
        let state = self.lock();
        if state.disposed {
            return Err(fault("fault.connection.closed"));
        }
        if !state.handshake_complete {
            return Err(fault("fault.connection.handshakeRequired"));
        }
        Ok(())
    }

    /// TS `beginPendingOwnership`。
    pub(super) fn begin_pending(&self, kind: TopicKind, params: &Value, topic: &str) -> u64 {
        let workspace = workspace_key(params);
        let mut state = self.lock();
        let id = state.next_pending;
        state.next_pending += 1;
        let generation = state.generations.get(&workspace).copied().unwrap_or(0);
        state.pending.insert(
            id,
            Pending {
                kind,
                group_key: format!("{}\0{workspace}\0{topic}", kind.as_str()),
                workspace_key: workspace,
                frames: Vec::new(),
                staged_bytes: 0,
                overflow: None,
                generation,
                invalidated: false,
            },
        );
        id
    }

    pub(super) fn discard_pending(&self, id: u64) {
        self.lock().pending.remove(&id);
    }

    /// TS `rememberAfterSubscribe` 的登记段：成功即 owner 生效并已按到达序释放暂存帧。
    /// 返回 `Err(reason)` 时调用方负责反向 unsubscribe（迟到 ACK 不能留下 runtime 侧订阅）。
    pub(super) fn remember_after_subscribe(
        &self,
        pending_id: u64,
        entry: Owned,
    ) -> Result<(), (&'static str, bool)> {
        let mut state = self.lock();
        let Some(pending) = state.pending.remove(&pending_id) else {
            // 已被失效逻辑摘除（runtime 换代）。
            return Err(("fault.subscription.runtimeRestarted", false));
        };
        let current = state
            .generations
            .get(&pending.workspace_key)
            .copied()
            .unwrap_or(0);
        if pending.invalidated || pending.generation != current {
            // 旧 runtime 的迟到 ACK 可能与新 runtime 复用同一 subId：只丢本地 owner，
            // 绝不向新 runtime 反向 unsubscribe（第二个元素 = 是否需要反向注销）。
            return Err(("fault.subscription.runtimeRestarted", false));
        }
        if state.disposed {
            return Err(("fault.connection.closed", true));
        }
        if let Some(reason) = pending.overflow {
            return Err((reason, true));
        }
        let route_key = route_key(&entry);
        if let Some(previous) = state
            .by_ownership
            .insert(entry.ownership_key.clone(), route_key.clone())
        {
            state.owned.remove(&previous);
        }
        state.owned.insert(route_key, entry.clone());
        // ACK 后只释放该 subscriptionId 的暂存帧，保持原到达顺序（CLI 帧可能先于 RPC 应答到达）。
        if let Some(route) = state
            .routes
            .get_mut(&(entry.kind, entry.workspace_key.clone()))
        {
            for frame in &pending.frames {
                if frame.subscription_id == entry.subscription_id && frame.topic == entry.topic {
                    route
                        .senders
                        .retain(|tx| tx.send(frame.params.clone()).is_ok());
                }
            }
        }
        Ok(())
    }

    pub(super) fn forget(&self, entry: &Owned) {
        let mut state = self.lock();
        let key = route_key(entry);
        state.owned.remove(&key);
        if state.by_ownership.get(&entry.ownership_key) == Some(&key) {
            state.by_ownership.remove(&entry.ownership_key);
        }
    }

    /// TS `findBySubscription`：同 (kind, workspace, subscriptionId) 唯一命中才返回（碰撞拒绝猜测）。
    pub(super) fn find(
        &self,
        kind: TopicKind,
        params: &Value,
        subscription_id: &str,
    ) -> Option<Owned> {
        let workspace = workspace_key(params);
        let state = self.lock();
        let mut matches = state.owned.values().filter(|entry| {
            entry.kind == kind
                && entry.subscription_id == subscription_id
                && entry.workspace_key == workspace
        });
        let first = matches.next()?.clone();
        matches.next().is_none().then_some(first)
    }

    pub(super) fn is_owned(&self, entry: &Owned) -> bool {
        self.lock().owned.contains_key(&route_key(entry))
    }

    /// 当前 flow 路由（owned 订阅 + attachment 路由），key → 路由 target。
    pub(super) fn flow_routes(&self) -> BTreeMap<String, Value> {
        let state = self.lock();
        let mut routes = state.attachment_routes.clone();
        for entry in state.owned.values() {
            routes.insert(
                flow_key(&entry.workspace_key, &self.connection_id),
                entry.target.clone(),
            );
        }
        routes
    }

    /// runtime 换代（`unavailable`）：本地 owner 直接 forget（不向新 runtime 清理），在飞 pending 失效。
    pub(crate) fn invalidate_runtime(&self, workspace: &str, generation: u64) {
        let mut state = self.lock();
        state.generations.insert(workspace.to_owned(), generation);
        let stale: Vec<String> = state
            .owned
            .iter()
            .filter(|(_, entry)| entry.workspace_key == workspace)
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            if let Some(entry) = state.owned.remove(&key)
                && state.by_ownership.get(&entry.ownership_key) == Some(&key)
            {
                state.by_ownership.remove(&entry.ownership_key);
            }
        }
        let prefix = format!("{workspace}\0");
        state
            .forwarded_flow
            .retain(|key, _| !key.starts_with(&prefix));
        for pending in state.pending.values_mut() {
            if pending.workspace_key == workspace {
                pending.invalidated = true;
                pending.frames.clear();
                pending.staged_bytes = 0;
            }
        }
    }

    /// 关闭：返回需要反向注销的 owner（调用方在锁外逐个 unsubscribe）。
    pub(super) fn take_for_dispose(&self) -> Option<Vec<Owned>> {
        let mut state = self.lock();
        if state.disposed {
            return None;
        }
        state.disposed = true;
        state.handshake_complete = false;
        state.hello_issued = false;
        state.bound_client_id = None;
        state.workflow_run_deltas = false;
        let entries = std::mem::take(&mut state.owned).into_values().collect();
        state.by_ownership.clear();
        state.pending.clear();
        for (_, route) in std::mem::take(&mut state.routes) {
            route.task.abort();
        }
        state.generations.clear();
        state.attachment_routes.clear();
        state.forwarded_flow.clear();
        Some(entries)
    }
}

pub(super) fn route_key(entry: &Owned) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        entry.kind.as_str(),
        entry.workspace_key,
        entry.topic,
        entry.subscription_id
    )
}

/// runtime 侧策略：入参 `runtimePolicy`。
pub(super) fn policy_of(params: &Value) -> RuntimePolicy {
    RuntimePolicy::of(params)
}
