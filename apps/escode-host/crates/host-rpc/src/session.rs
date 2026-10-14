//! RPC 会话：`ChannelServer`/`ChannelClient` 的 Rust 侧对端（packages/rpc/src/channelServer.ts）。
//!
//! 连接建立即发 `Initialize`（`[200]` + Undefined）；随后按请求类型处理：
//!
//! | 请求（RequestType） | 行为 |
//! | --- | --- |
//! | `Promise`(100) | 查 channel 表；命中即执行并回 `[201, id]` + 数据；失败回 `[202, id]` + 结构化错误 |
//! | `PromiseCancel`(101) / `EventDispose`(103) | 释放该 id（当前无长任务，等价 no-op） |
//! | `EventListen`(102) | 回结构化错误（事件面尚未迁移，缺口显式） |
//!
//! 未迁移的 channel 回 `NotImplemented` 错误（Renderer 见显式缺口，不挂起）。
//! channel 名来自 TS `ServiceChannels`（`packages/shared/src/channels.ts`）；方法名在请求头第 4 项。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
}

/// 把一个闭包包装为方法处理函数（表项用）。
pub fn handler<F>(f: F) -> MethodHandler
where
    F: Fn(Wire) -> Pin<Box<dyn Future<Output = ChannelResult> + Send>> + Send + Sync + 'static,
{
    Arc::new(f)
}

impl Channel {
    pub fn new(name: &'static str, methods: Vec<(&'static str, MethodHandler)>) -> Self {
        Self { name, methods }
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
    entries: Vec<Channel>,
}

impl Channels {
    pub fn new(entries: Vec<Channel>) -> Self {
        Self { entries }
    }

    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.entries.iter().find(|channel| channel.name == name)
    }

    /// 已登记的 channel 名（能力清单与诊断用）。
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|channel| channel.name).collect()
    }

    /// 已实现集合（channel, method）——契约清单对账与 `/api/server-info` 用。
    pub fn inventory(&self) -> Vec<(&'static str, &'static str)> {
        self.entries
            .iter()
            .flat_map(|channel| {
                channel
                    .methods()
                    .into_iter()
                    .map(move |method| (channel.name, method))
            })
            .collect()
    }
}

/// 处理一条请求，返回要回给客户端的响应 body（可能为空：取消类请求无响应）。
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
        REQUEST_EVENT_LISTEN => error_frame(
            request.id,
            "NotImplemented",
            &format!(
                "Host does not implement events for channel `{}` yet",
                request.channel
            ),
        ),
        // 取消/注销：没有长任务与事件订阅，等价 no-op（TS 客户端按需忽略）。
        REQUEST_PROMISE_CANCEL | REQUEST_EVENT_DISPOSE => Vec::new(),
        other => error_frame(
            request.id,
            "InvalidRequest",
            &format!("Unsupported RPC request type {other}"),
        ),
    }
}
