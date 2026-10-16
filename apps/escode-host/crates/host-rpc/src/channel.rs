//! channel 注册表与连接上下文（从 `session.rs` 拆出以守住单文件行数上限；`session::*` 原样再导出）。
//!
//! - [`Channel`]：方法表 + 事件表即分派本身（单一来源，契约清单双向对账）；
//! - [`Channels`]：共享 channel + 连接级 channel 工厂（每条连接实例化一份）；
//! - 打开/关闭钩子：连接级 channel 的后台任务只在真实连接上启动（探测方法表不产生副作用）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::stream::BoxStream;

use crate::session::{ChannelResult, RpcError, RpcResult};
use crate::wire::Wire;

/// 单个方法的处理函数（入参 → 结果）。方法名由所属 [`Channel`] 的表键给出。
pub type MethodHandler =
    Arc<dyn Fn(Wire) -> Pin<Box<dyn Future<Output = RpcResult> + Send>> + Send + Sync>;

/// 一个 channel：方法表即分派本身（**单一来源**）。
///
/// 契约清单（`host_proto::METHODS`）与 `host-rpc` 的实现集合由
/// `tests/contract.rs` 双向对账：表里多出的方法在清单里必须是 `Implemented`，
/// 清单里标 `Implemented` 的方法必须能在表里找到——"缺口显式可见"因此是机器校验的。
pub struct Channel {
    pub name: &'static str,
    methods: Vec<(&'static str, MethodHandler)>,
    events: Vec<(&'static str, EventHandler)>,
    on_open: Option<OpenHook>,
    on_close: Option<CloseHook>,
}

/// 事件流：每个元素是一次 `EventFire`；丢弃即注销（`EventDispose` / 连接关闭）。
pub type EventStream = BoxStream<'static, Wire>;
/// 事件订阅函数：`arg` 是 `onDynamicX(arg)` 的实参（静态事件为 Undefined）。
/// 失败只记日志、不回响应（TS 同）。
pub type EventHandler = Arc<dyn Fn(Wire) -> Result<EventStream, String> + Send + Sync>;
/// 连接打开钩子（连接级 channel 的后台任务在这里启动；只由连接循环调用，探测方法表不调用）。
pub type OpenHook = Arc<dyn Fn() + Send + Sync>;
/// 连接关闭钩子（连接级 channel 的清理，如 V4 订阅注销）。
pub type CloseHook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// 连接的交付模式（TS `ESCodeAgentV4ClientMode`）：由连接路径决定，Renderer 不能自选。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientMode {
    DesktopContinuous,
    WebRemoteReplayable,
}

impl ClientMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DesktopContinuous => "desktop-continuous",
            Self::WebRemoteReplayable => "web-remote-replayable",
        }
    }
}

/// 连接出站水位状态（TS `V4ConnectionFlowState`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowState {
    Drained,
    Saturated,
    Closed,
}

impl FlowState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drained => "drained",
            Self::Saturated => "saturated",
            Self::Closed => "closed",
        }
    }
}

/// 一条 RPC 连接的可信上下文（Host 生成；连接级 channel 工厂据此建作用域）。
#[derive(Clone)]
pub struct ConnectionContext {
    pub connection_id: String,
    pub client_mode: ClientMode,
    /// 出站水位边沿（`connection.rs` 维护）。
    pub flow: tokio::sync::watch::Receiver<FlowState>,
}

impl ConnectionContext {
    /// 新连接上下文：`host-rpc-<uuid v4>`（TS `exposeServicesOnMessagePort` 同形）。
    pub fn new(client_mode: ClientMode, flow: tokio::sync::watch::Receiver<FlowState>) -> Self {
        Self {
            connection_id: format!("host-rpc-{}", uuid::Uuid::new_v4()),
            client_mode,
            flow,
        }
    }
}

/// 连接级 channel 工厂：每条连接建一个独立实例（TS `createESCodeAgentConnectionScope`）。
pub type ChannelFactory = Arc<dyn Fn(&ConnectionContext) -> Channel + Send + Sync>;

/// 把一个闭包包装为方法处理函数（表项用；错误为 `(name, message)`）。
pub fn handler<F>(f: F) -> MethodHandler
where
    F: Fn(Wire) -> Pin<Box<dyn Future<Output = ChannelResult> + Send>> + Send + Sync + 'static,
{
    Arc::new(move |arg| {
        let future = f(arg);
        Box::pin(async move { future.await.map_err(RpcError::from) })
    })
}

/// 同上，但闭包直接产出结构化错误（需要透传 `code` 等键时用）。
pub fn handler_rpc<F>(f: F) -> MethodHandler
where
    F: Fn(Wire) -> Pin<Box<dyn Future<Output = RpcResult> + Send>> + Send + Sync + 'static,
{
    Arc::new(f)
}

impl Channel {
    pub fn new(name: &'static str, methods: Vec<(&'static str, MethodHandler)>) -> Self {
        Self {
            name,
            methods,
            events: Vec::new(),
            on_open: None,
            on_close: None,
        }
    }

    /// 追加方法（同一 channel 的方法分散在多个域文件时用；后追加的同名项不会覆盖先登记的）。
    pub fn with_methods(mut self, extra: Vec<(&'static str, MethodHandler)>) -> Self {
        self.methods.extend(extra);
        self
    }

    /// 登记事件表（`onX` 静态事件与 `onDynamicX` 动态事件同表）。
    pub fn with_events(mut self, events: Vec<(&'static str, EventHandler)>) -> Self {
        self.events = events;
        self
    }

    /// 登记连接打开钩子。
    pub fn with_open(mut self, hook: OpenHook) -> Self {
        self.on_open = Some(hook);
        self
    }

    /// 登记连接关闭钩子。
    pub fn with_close(mut self, hook: CloseHook) -> Self {
        self.on_close = Some(hook);
        self
    }

    pub fn event(&self, name: &str) -> Option<EventHandler> {
        self.events
            .iter()
            .find(|(event, _)| *event == name)
            .map(|(_, handler)| Arc::clone(handler))
    }

    /// 该 channel 已实现的事件名（按声明序）。
    pub fn events(&self) -> Vec<&'static str> {
        self.events.iter().map(|(name, _)| *name).collect()
    }

    pub(crate) fn open_hook(&self) -> Option<OpenHook> {
        self.on_open.clone()
    }

    pub(crate) fn close_hook(&self) -> Option<CloseHook> {
        self.on_close.clone()
    }

    pub fn handler(&self, method: &str) -> Option<MethodHandler> {
        self.methods
            .iter()
            .find(|(name, _)| *name == method)
            .map(|(_, handler)| Arc::clone(handler))
    }

    /// 该 channel 已实现的方法名（按声明序）。
    pub fn methods(&self) -> Vec<&'static str> {
        self.methods.iter().map(|(name, _)| *name).collect()
    }
}

/// channel 表：已迁移的服务域登记在这里；未登记即显式缺口。
#[derive(Default)]
pub struct Channels {
    entries: Vec<Arc<Channel>>,
    scoped: Vec<ChannelFactory>,
}

impl Channels {
    pub fn new(entries: Vec<Channel>) -> Self {
        Self {
            entries: entries.into_iter().map(Arc::new).collect(),
            scoped: Vec::new(),
        }
    }

    /// 追加一个连接级 channel（每条连接由工厂各建一份）。
    pub fn with_scoped(mut self, factory: ChannelFactory) -> Self {
        self.scoped.push(factory);
        self
    }

    /// 为一条连接实例化：共享 channel 复用，连接级 channel 由工厂新建。
    pub fn connect(&self, context: &ConnectionContext) -> Channels {
        let mut entries = self.entries.clone();
        entries.extend(self.scoped.iter().map(|factory| Arc::new(factory(context))));
        Channels {
            entries,
            scoped: Vec::new(),
        }
    }

    /// 连接打开钩子（按登记序）。
    pub(crate) fn open_hooks(&self) -> Vec<OpenHook> {
        self.entries
            .iter()
            .filter_map(|channel| channel.open_hook())
            .collect()
    }

    /// 连接关闭钩子（按登记序）。
    pub(crate) fn close_hooks(&self) -> Vec<CloseHook> {
        self.entries
            .iter()
            .filter_map(|channel| channel.close_hook())
            .collect()
    }

    /// 连接级 channel 的方法表要实例化才能列出：用一个与连接无关的上下文建一份（只读方法名）。
    fn probe_scoped(&self) -> Vec<Channel> {
        let (_tx, flow) = tokio::sync::watch::channel(FlowState::Drained);
        let context = ConnectionContext::new(ClientMode::DesktopContinuous, flow);
        self.scoped
            .iter()
            .map(|factory| factory(&context))
            .collect()
    }

    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.entries
            .iter()
            .find(|channel| channel.name == name)
            .map(AsRef::as_ref)
    }

    /// 已登记的 channel 名（能力清单与诊断用；含连接级 channel）。
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> =
            self.entries.iter().map(|channel| channel.name).collect();
        names.extend(self.probe_scoped().iter().map(|channel| channel.name));
        names
    }

    /// 已实现集合（channel, method）——契约清单对账与 `/api/server-info` 用。
    /// 事件与方法同表登记（TS 服务接口里事件也是成员），一并列出。
    pub fn inventory(&self) -> Vec<(&'static str, &'static str)> {
        let probed = self.probe_scoped();
        self.entries
            .iter()
            .map(AsRef::as_ref)
            .chain(probed.iter())
            .flat_map(|channel| {
                channel
                    .methods()
                    .into_iter()
                    .chain(channel.events())
                    .map(move |member| (channel.name, member))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}
