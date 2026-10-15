//! RPC 会话：`ChannelServer`/`ChannelClient` 的 Rust 侧对端（packages/rpc/src/channelServer.ts）。
//!
//! 连接建立即发 `Initialize`（`[200]` + Undefined）；随后按请求类型处理：
//!
//! | 请求（RequestType） | 行为 |
//! | --- | --- |
//! | `Promise`(100) | 查 channel 表；命中即执行并回 `[201, id]` + 数据；失败回 `[202, id]` + 结构化错误 |
//! | `PromiseCancel`(101) | 中止该请求（不回响应；TS 客户端已自行 reject） |
//! | `EventListen`(102) | 查 channel 事件表；命中即订阅，每个值回 `[204, id]` + 数据 |
//! | `EventDispose`(103) | 取消该订阅 |
//!
//! 每条请求独立执行（连接循环见 `connection.rs`）。未迁移的 channel/方法回 `NotImplemented`
//! 错误（Renderer 见显式缺口，不挂起）；未知事件不回响应（TS `ChannelServer` 同：事件没有错误帧）。
//! channel 名来自 TS `ServiceChannels`（`packages/shared/src/channels.ts`）；方法名在请求头第 4 项。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::stream::BoxStream;
use serde_json::json;

use crate::wire::{Reader, Wire, Writer};

/// 请求类型（TS `RequestType`）。
pub const REQUEST_PROMISE: i64 = 100;
pub const REQUEST_PROMISE_CANCEL: i64 = 101;
pub const REQUEST_EVENT_LISTEN: i64 = 102;
pub const REQUEST_EVENT_DISPOSE: i64 = 103;

/// 响应类型（TS `ResponseType`）。
pub const RESPONSE_INITIALIZE: i64 = 200;
pub const RESPONSE_PROMISE_SUCCESS: i64 = 201;
pub const RESPONSE_PROMISE_ERROR: i64 = 202;
pub const RESPONSE_EVENT_FIRE: i64 = 204;

/// 一条已解析的请求。
#[derive(Debug, PartialEq)]
pub struct Request {
    pub kind: i64,
    pub id: i64,
    pub channel: String,
    pub method: String,
    pub arg: Wire,
}

/// 编码 `Initialize` 响应（连接建立后服务端的第一帧）。
pub fn initialize_frame() -> Vec<u8> {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![Wire::Int(RESPONSE_INITIALIZE)]));
    writer.value(&Wire::Undefined);
    writer.into_bytes()
}

/// 编码 `PromiseSuccess`。
pub fn success_frame(id: i64, data: &Wire) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(RESPONSE_PROMISE_SUCCESS),
        Wire::Int(id),
    ]));
    writer.value(data);
    writer.into_bytes()
}

/// 编码 `EventFire`（TS `ChannelServer.onEventListen`：`{id, data, type: EventFire}`）。
pub fn event_frame(id: i64, data: &Wire) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(RESPONSE_EVENT_FIRE),
        Wire::Int(id),
    ]));
    writer.value(data);
    writer.into_bytes()
}

/// 编码 `PromiseError`（TS 的 `{message, name, stack}` 形状）。
pub fn error_frame(id: i64, name: &str, message: &str) -> Vec<u8> {
    let payload = json!({
        "message": message,
        "name": name,
        "stack": [message],
    });
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(RESPONSE_PROMISE_ERROR),
        Wire::Int(id),
    ]));
    writer.value(&Wire::Object(payload));
    writer.into_bytes()
}

/// 解析一条请求帧（`serialize([type, id, channel, method])` + `serialize(arg)`）。
pub fn parse_request(body: &[u8]) -> anyhow::Result<Request> {
    let mut reader = Reader::new(body);
    let header = reader.value()?;
    let arg = reader.value()?;
    let Wire::Array(items) = header else {
        anyhow::bail!("RPC request header must be an array");
    };
    anyhow::ensure!(items.len() >= 2, "RPC request header is too short");
    let kind = items[0]
        .as_int()
        .ok_or_else(|| anyhow::anyhow!("RPC request type must be an integer"))?;
    let id = items[1]
        .as_int()
        .ok_or_else(|| anyhow::anyhow!("RPC request id must be an integer"))?;
    Ok(Request {
        kind,
        id,
        channel: items
            .get(2)
            .and_then(Wire::as_str)
            .unwrap_or_default()
            .to_owned(),
        method: items
            .get(3)
            .and_then(Wire::as_str)
            .unwrap_or_default()
            .to_owned(),
        arg,
    })
}

/// 处理结果：成功数据，或 `(错误名, 消息)`。
pub type ChannelResult = Result<Wire, (String, String)>;
/// 单个方法的处理函数（入参 → 结果）。方法名由所属 [`Channel`] 的表键给出。
pub type MethodHandler =
    Arc<dyn Fn(Wire) -> Pin<Box<dyn Future<Output = ChannelResult> + Send>> + Send + Sync>;

/// 一个 channel：方法表即分派本身（**单一来源**）。
///
/// 契约清单（`host_proto::METHODS`）与 `host-rpc` 的实现集合由
/// `tests/contract.rs` 双向对账：表里多出的方法在清单里必须是 `Implemented`，
/// 清单里标 `Implemented` 的方法必须能在表里找到——"缺口显式可见"因此是机器校验的。
pub struct Channel {
    pub name: &'static str,
    methods: Vec<(&'static str, MethodHandler)>,
    events: Vec<(&'static str, EventHandler)>,
    on_close: Option<CloseHook>,
}

/// 事件流：每个元素是一次 `EventFire`；丢弃即注销（`EventDispose` / 连接关闭）。
pub type EventStream = BoxStream<'static, Wire>;
/// 事件订阅函数：`arg` 是 `onDynamicX(arg)` 的实参（静态事件为 Undefined）。
/// 失败只记日志、不回响应（TS 同）。
pub type EventHandler = Arc<dyn Fn(Wire) -> Result<EventStream, String> + Send + Sync>;
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

/// 把一个闭包包装为方法处理函数（表项用）。
pub fn handler<F>(f: F) -> MethodHandler
where
    F: Fn(Wire) -> Pin<Box<dyn Future<Output = ChannelResult> + Send>> + Send + Sync + 'static,
{
    Arc::new(f)
}

impl Channel {
    pub fn new(name: &'static str, methods: Vec<(&'static str, MethodHandler)>) -> Self {
        Self {
            name,
            methods,
            events: Vec::new(),
            on_close: None,
        }
    }

    /// 登记事件表（`onX` 静态事件与 `onDynamicX` 动态事件同表）。
    pub fn with_events(mut self, events: Vec<(&'static str, EventHandler)>) -> Self {
        self.events = events;
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

/// 处理一条 `Promise` 请求，返回要回给客户端的响应 body（其余类型返回空）。
pub async fn respond(channels: &Channels, request: &Request) -> Vec<u8> {
    match request.kind {
        REQUEST_PROMISE => match channels.channel(&request.channel) {
            Some(channel) => match channel.handler(&request.method) {
                Some(handler) => match handler(request.arg.clone()).await {
                    Ok(data) => success_frame(request.id, &data),
                    Err((name, message)) => error_frame(request.id, &name, &message),
                },
                None => error_frame(
                    request.id,
                    "NotImplemented",
                    &format!(
                        "Host does not implement `{}.{}` yet (implemented: {})",
                        request.channel,
                        request.method,
                        channel.methods().join(", ")
                    ),
                ),
            },
            None => error_frame(
                request.id,
                "NotImplemented",
                &format!(
                    "Host does not implement channel `{}` yet (implemented: {})",
                    request.channel,
                    channels.names().join(", ")
                ),
            ),
        },
        // 事件与取消由连接循环处理（`connection.rs`）；单独调用 `respond` 时不产生响应。
        REQUEST_EVENT_LISTEN | REQUEST_PROMISE_CANCEL | REQUEST_EVENT_DISPOSE => Vec::new(),
        other => error_frame(
            request.id,
            "InvalidRequest",
            &format!("Unsupported RPC request type {other}"),
        ),
    }
}
