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

use std::any::Any;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
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
    rpc_error_frame(id, &RpcError::new(name, message))
}

/// 结构化 RPC 错误（TS `ChannelServer.onPromise` 的 `PromiseError` 载荷）：`name`/`message` 之外，
/// TS 透传 `code`/`kind`/`status`/`retryAfterMs`/`data`/`detail`/`details`/`taskId`/`traceId`
/// （`channelServer.ts:156`）。例：existing-only 无运行时回 `code: "ESCODE_AGENT_RUNTIME_UNAVAILABLE"`。
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub name: String,
    pub message: String,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl RpcError {
    pub fn new(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message: message.into(),
            extra: serde_json::Map::new(),
        }
    }

    /// 附一个 TS 透传键（只接受白名单内的键，与 TS 同口径）。
    pub fn with(mut self, key: &'static str, value: serde_json::Value) -> Self {
        const PASSTHROUGH: [&str; 9] = [
            "code",
            "kind",
            "status",
            "retryAfterMs",
            "data",
            "detail",
            "details",
            "taskId",
            "traceId",
        ];
        if PASSTHROUGH.contains(&key) {
            self.extra.insert(key.to_owned(), value);
        }
        self
    }
}

impl From<(String, String)> for RpcError {
    fn from((name, message): (String, String)) -> Self {
        Self::new(name, message)
    }
}

/// 退化为 `(name, message)`（旧式处理函数用；透传键丢失——需要保真的路径改用 [`handler_rpc`]）。
impl From<RpcError> for (String, String) {
    fn from(error: RpcError) -> Self {
        (error.name, error.message)
    }
}

/// 编码结构化 `PromiseError`。
pub fn rpc_error_frame(id: i64, error: &RpcError) -> Vec<u8> {
    let mut payload = json!({
        "message": error.message,
        "name": error.name,
        "stack": [error.message],
    });
    if let Some(record) = payload.as_object_mut() {
        for (key, value) in &error.extra {
            record.insert(key.clone(), value.clone());
        }
    }
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
/// 带结构化错误的处理结果（[`MethodHandler`] 的统一出口）。
pub type RpcResult = Result<Wire, RpcError>;
pub use crate::channel::*;

/// 一条请求的响应 future（`'static`：可直接交给独立任务）。
pub type Responding = std::pin::Pin<Box<dyn std::future::Future<Output = Vec<u8>> + Send>>;

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_owned())
}

/// **同步**登记一条请求：在调用线程查 channel/method 并调用处理函数（处理函数的同步前缀此刻执行），
/// 返回产出响应 body 的 future（其余类型产出空 body）。
///
/// 修复：连接读循环过去在独立任务里才调用处理函数，tokio 多线程调度下同一连接先后两条请求的
/// 处理函数调用顺序不确定；TS `ChannelServer` 收到 Promise 请求即同步 `channel.call`（登记序 =
/// 到达序）。读循环按到达序调用本函数，再把 future 交给独立任务（docs/specs/rust-u2-host-resilience.md §5.1）。
pub fn start(channels: &Channels, request: &Request) -> Responding {
    let id = request.id;
    let ready = |body: Vec<u8>| -> Responding { Box::pin(std::future::ready(body)) };
    match request.kind {
        REQUEST_PROMISE => match channels.channel(&request.channel) {
            Some(channel) => match channel.handler(&request.method) {
                Some(handler) => {
                    // 修复：处理函数的同步前缀或异步体 panic 时，原实现会让独立任务直接展开，
                    // 请求既无成功帧也无错误帧，客户端只能等到自己的超时。两段都转成 `Fatal`
                    // 错误帧，保证每条 Promise 请求都有应答（docs/specs/rust-u2-host-resilience.md §5.1）。
                    let label = format!("`{}.{}`", request.channel, request.method);
                    let pending = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                        handler(request.arg.clone())
                    })) {
                        Ok(pending) => pending,
                        Err(payload) => {
                            return ready(error_frame(
                                id,
                                "Fatal",
                                &format!(
                                    "Host handler {label} panicked: {}",
                                    panic_message(payload.as_ref())
                                ),
                            ));
                        }
                    };
                    Box::pin(async move {
                        match AssertUnwindSafe(pending).catch_unwind().await {
                            Ok(Ok(data)) => success_frame(id, &data),
                            Ok(Err(error)) => rpc_error_frame(id, &error),
                            Err(payload) => rpc_error_frame(
                                id,
                                &RpcError::new(
                                    "Fatal",
                                    format!(
                                        "Host handler {label} panicked: {}",
                                        panic_message(payload.as_ref())
                                    ),
                                ),
                            ),
                        }
                    })
                }
                None => ready(error_frame(
                    id,
                    "NotImplemented",
                    &format!(
                        "Host does not implement `{}.{}` yet (implemented: {})",
                        request.channel,
                        request.method,
                        channel.methods().join(", ")
                    ),
                )),
            },
            None => ready(error_frame(
                id,
                "NotImplemented",
                &format!(
                    "Host does not implement channel `{}` yet (implemented: {})",
                    request.channel,
                    channels.names().join(", ")
                ),
            )),
        },
        // 事件与取消由连接循环处理（`connection.rs`）；单独调用时不产生响应。
        REQUEST_EVENT_LISTEN | REQUEST_PROMISE_CANCEL | REQUEST_EVENT_DISPOSE => ready(Vec::new()),
        other => ready(error_frame(
            id,
            "InvalidRequest",
            &format!("Unsupported RPC request type {other}"),
        )),
    }
}

/// 处理一条 `Promise` 请求，返回要回给客户端的响应 body（其余类型返回空）。
pub async fn respond(channels: &Channels, request: &Request) -> Vec<u8> {
    start(channels, request).await
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
