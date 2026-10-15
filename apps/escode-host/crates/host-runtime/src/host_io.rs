//! 实例与 Host 之间的两类「非应答」交互（docs/specs/rust-u2-agent-v4.md §2 / §3.3）：
//!
//! - **反向请求**（runtime→Host request，帧形 `{id, method, params}`）：由 Host 注入的
//!   [`HostResponder`] 应答；未注入或未识别的方法回 `-32601 Unsupported ESCode Protocol request: <method>`
//!   （TS `escodeAgentService.ts` 的 `client.onRequest` 兜底同文）。此前 Rust Host 不应答反向请求，
//!   runtime 等待 `session/requestRuntimePreferences` 回包的路径会一直挂起。
//! - **生命周期事件**：实例登记即 `available`，读端关闭（退出/崩溃/收口）即 `unavailable`
//!   （TS `onAgentRuntimeLifecycle` 同义）。订阅方据此丢弃该代的订阅归属。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::mpsc;

/// 一次反向请求。`identity` = workspace 身份键（`workspaceIdentity?.trim() || workspacePath`）。
#[derive(Clone, Debug)]
pub struct HostRequest {
    pub identity: String,
    pub workspace_path: String,
    pub method: String,
    pub params: Value,
}

/// 反向请求的错误应答（JSON-RPC error 形状）。
#[derive(Clone, Debug, PartialEq)]
pub struct HostError {
    pub code: i64,
    pub message: String,
}

impl HostError {
    /// TS 兜底：未识别的反向请求。
    pub fn unsupported(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Unsupported ESCode Protocol request: {method}"),
        }
    }
}

pub type HostReply = Pin<Box<dyn Future<Output = Result<Value, HostError>> + Send>>;
/// Host 侧反向请求应答器（组合根注入；Host 内唯一）。
pub type HostResponder = Arc<dyn Fn(HostRequest) -> HostReply + Send + Sync>;

/// 应答器槽位：实例表与各实例共享，按请求读取当前应答器。
#[derive(Clone, Default)]
pub(crate) struct ResponderSlot(Arc<std::sync::RwLock<Option<HostResponder>>>);

impl ResponderSlot {
    pub(crate) fn set(&self, responder: HostResponder) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(responder);
    }

    pub(crate) fn current(&self) -> Option<HostResponder> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 生命周期状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Available,
    Unavailable,
}

/// 实例生命周期事件（TS `ESCodeAgentRuntimeLifecycleEvent` 的字段来源）。
#[derive(Clone, Debug, PartialEq)]
pub struct LifecycleEvent {
    pub identity: String,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub generation: u64,
    /// 不透明运行时身份：`<workspaceKey>:<generation>:<pid | "embedded">`（TS 用子进程 pid）。
    pub runtime_identity: String,
    pub process_id: Option<u32>,
    pub state: RuntimeState,
}

/// 生命周期事件扇出。
///
/// 溢出策略：无界（事件条数受实例启停次数约束，每代恰好两条）；接收端关闭即摘除。
#[derive(Default)]
pub(crate) struct LifecycleHub {
    listeners: Mutex<Vec<mpsc::UnboundedSender<LifecycleEvent>>>,
}

impl LifecycleHub {
    pub(crate) fn listen(&self) -> mpsc::UnboundedReceiver<LifecycleEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().push(tx);
        rx
    }

    pub(crate) fn publish(&self, event: &LifecycleEvent) {
        self.lock()
            .retain(|listener| listener.send(event.clone()).is_ok());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<mpsc::UnboundedSender<LifecycleEvent>>> {
        self.listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
