//! 远端 server 的 RPC 客户端（与 `packages/rpc/src/channelClient.ts` 对等的最小实现）。
//!
//! 远端 stdin 收 [`Self::call`] / [`Self::listen`] 生成的帧，stdout 的 Regular 载荷交给
//! [`Self::handle_frame`]。Initialize（能力广播）之前发起的请求会等待握手完成，与 TS 一致。
//!
//! 这一层不关心 channel 语义：`source-server-management` / `escode-agent` 等都由同名
//! channel 直接转发，业务投影仍在上层。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Result, bail};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use escode_host_core::rpc_serialization::WireValue;
use escode_host_core::rpc_wire::encode_regular;

pub const REQUEST_PROMISE: i32 = 100;
pub const REQUEST_PROMISE_CANCEL: i32 = 101;
pub const REQUEST_EVENT_LISTEN: i32 = 102;
pub const REQUEST_EVENT_DISPOSE: i32 = 103;

pub const RESPONSE_INITIALIZE: i32 = 200;
pub const RESPONSE_PROMISE_SUCCESS: i32 = 201;
pub const RESPONSE_PROMISE_ERROR: i32 = 202;
pub const RESPONSE_PROMISE_ERROR_OBJ: i32 = 203;
pub const RESPONSE_EVENT_FIRE: i32 = 204;

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteCallError {
    pub message: String,
    pub name: String,
    /// PromiseError 的原始 data（message/name/stack/code/...），PromiseErrorObj 的 body。
    pub data: Option<Value>,
}

impl RemoteCallError {
    pub fn closed() -> Self {
        Self {
            message: "remote channel client disposed".to_owned(),
            name: "ConnectionClosed".to_owned(),
            data: None,
        }
    }

    fn from_body(body: &WireValue) -> Self {
        let Some(value) = body.to_json() else {
            return Self {
                message: "remote call failed".to_owned(),
                name: "Error".to_owned(),
                data: None,
            };
        };
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("remote call failed")
            .to_owned();
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Error")
            .to_owned();
        Self {
            message,
            name,
            data: Some(value),
        }
    }
}

impl std::fmt::Display for RemoteCallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for RemoteCallError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitState {
    Pending,
    Ready,
    Closed,
}

type PendingSender = oneshot::Sender<Result<WireValue, RemoteCallError>>;
type EventSender = mpsc::UnboundedSender<WireValue>;

struct ClientState {
    init: InitState,
    channels: Option<HashSet<String>>,
    pending: HashMap<u32, PendingSender>,
    events: HashMap<u32, EventSender>,
}

pub struct RemoteChannelClient {
    /// 出站完整线帧（含 13 字节 Regular 头），transport 直接写入链路。
    outbound: mpsc::UnboundedSender<Vec<u8>>,
    init_tx: watch::Sender<InitState>,
    state: Mutex<ClientState>,
    last_request_id: AtomicU32,
}

impl RemoteChannelClient {
    pub fn new(outbound: mpsc::UnboundedSender<Vec<u8>>) -> Self {
        let (init_tx, _) = watch::channel(InitState::Pending);
        Self {
            outbound,
            init_tx,
            state: Mutex::new(ClientState {
                init: InitState::Pending,
                channels: None,
                pending: HashMap::new(),
                events: HashMap::new(),
            }),
            last_request_id: AtomicU32::new(0),
        }
    }

    /// 等待远端 Initialize（能力广播随之到达）。
    pub async fn wait_initialized(&self) -> Result<()> {
        let mut receiver = self.init_tx.subscribe();
        loop {
            if *receiver.borrow() == InitState::Ready {
                return Ok(());
            }
            if *receiver.borrow() == InitState::Closed {
                return Err(RemoteCallError::closed().into());
            }
            if receiver.changed().await.is_err() {
                return Err(RemoteCallError::closed().into());
            }
        }
    }

    /// 等待连接终结（`dispose` 已发生则立即返回）。
    ///
    /// scope RPC 连接用它把「远端链路 EOF」传导成 WebSocket 关闭，而不是让窗口继续挂着失效句柄
    /// （docs/specs/source-server-remote-rust-host.md §10.14）。
    pub async fn wait_closed(&self) {
        let mut receiver = self.init_tx.subscribe();
        loop {
            if *receiver.borrow_and_update() == InitState::Closed {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }

    /// 远端广播过该 channel 才返回 true；未广播（旧 Host）按不可用处理。
    pub fn has_channel(&self, channel: &str) -> bool {
        self.lock()
            .channels
            .as_ref()
            .is_some_and(|channels| channels.contains(channel))
    }

    pub fn channel_names(&self) -> Option<HashSet<String>> {
        self.lock().channels.clone()
    }

    /// 发起一次 Promise 请求；Initialize 之前调用会等待握手。
    pub async fn call(
        &self,
        channel: &str,
        method: &str,
        arg: Option<WireValue>,
    ) -> Result<WireValue> {
        self.wait_initialized().await?;
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.lock();
            if state.init != InitState::Ready {
                return Err(RemoteCallError::closed().into());
            }
            state.pending.insert(id, tx);
        }
        self.send_request(REQUEST_PROMISE, id, channel, Some(method), arg);
        match rx.await {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(RemoteCallError::closed().into()),
        }
    }

    /// 监听远端事件；返回的接收端丢弃后调用 [`Self::dispose_listener`] 取消订阅。
    pub async fn listen(
        &self,
        channel: &str,
        event: &str,
        arg: Option<WireValue>,
    ) -> Result<(u32, mpsc::UnboundedReceiver<WireValue>)> {
        self.wait_initialized().await?;
        Ok(self.register_listener(channel, event, arg))
    }

    /// 已 `Initialize` 时**同步**登记并发送 EventListen；未握手完成返回 `None`。
    ///
    /// 远端 scope 的事件转发必须在处理 listen 帧的同一调用栈里用它建流：Host 读循环对每个帧
    /// 都 `tokio::spawn` 处理，惰性建流会让紧随其后的 `call`（如 `subscribeConversationV4`）
    /// 先到远端；远端随后立刻投递、不重发的帧就永久丢失。见 §10.11 的 2026-11-01 记录。
    pub fn listen_now(
        &self,
        channel: &str,
        event: &str,
        arg: Option<WireValue>,
    ) -> Option<(u32, mpsc::UnboundedReceiver<WireValue>)> {
        if self.lock().init != InitState::Ready {
            return None;
        }
        Some(self.register_listener(channel, event, arg))
    }

    fn register_listener(
        &self,
        channel: &str,
        event: &str,
        arg: Option<WireValue>,
    ) -> (u32, mpsc::UnboundedReceiver<WireValue>) {
        let id = self.next_id();
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().events.insert(id, tx);
        self.send_request(REQUEST_EVENT_LISTEN, id, channel, Some(event), arg);
        (id, rx)
    }

    pub fn dispose_listener(&self, id: u32) {
        self.lock().events.remove(&id);
        self.send_raw(REQUEST_EVENT_DISPOSE, id, None);
    }

    /// 处理一条已分帧的 Regular 载荷；返回是否有 handler 消费。
    pub fn handle_frame(&self, payload: &[u8]) -> Result<()> {
        let (header, used) = WireValue::decode(payload)?;
        let (body, _) = WireValue::decode(&payload[used..])?;
        let WireValue::Array(items) = header else {
            bail!("remote response header is not an array");
        };
        let Some(WireValue::Int(kind)) = items.first() else {
            bail!("remote response header missing type");
        };
        let id = items.get(1).and_then(|item| match item {
            WireValue::Int(id) => u32::try_from(*id).ok(),
            _ => None,
        });
        match *kind {
            RESPONSE_INITIALIZE => self.on_initialize(&body),
            RESPONSE_PROMISE_SUCCESS => {
                if let Some(id) = id
                    && let Some(tx) = self.lock().pending.remove(&id)
                {
                    let _ = tx.send(Ok(body));
                }
            }
            RESPONSE_PROMISE_ERROR => {
                if let Some(id) = id
                    && let Some(tx) = self.lock().pending.remove(&id)
                {
                    let _ = tx.send(Err(RemoteCallError::from_body(&body)));
                }
            }
            RESPONSE_PROMISE_ERROR_OBJ => {
                if let Some(id) = id
                    && let Some(tx) = self.lock().pending.remove(&id)
                {
                    let _ = tx.send(Err(RemoteCallError::from_body(&body)));
                }
            }
            RESPONSE_EVENT_FIRE => {
                if let Some(id) = id
                    && let Some(sender) = self.lock().events.get(&id).cloned()
                {
                    let _ = sender.send(body);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 连接终结：拒绝所有挂起请求，并让 Initialize 等待者失败。
    pub fn dispose(&self) {
        let (pending, events) = {
            let mut state = self.lock();
            state.init = InitState::Closed;
            state.channels = None;
            (
                std::mem::take(&mut state.pending),
                std::mem::take(&mut state.events),
            )
        };
        // 用 `send_replace`：初始化可能发生在任何订阅者出现之前，`send` 在没有
        // receiver 时不会更新值，后来者会永远等不到 Ready/Closed。
        self.init_tx.send_replace(InitState::Closed);
        for (_, sender) in pending {
            let _ = sender.send(Err(RemoteCallError::closed()));
        }
        drop(events);
    }

    fn on_initialize(&self, body: &WireValue) {
        let channels = match body.to_json() {
            Some(Value::Object(map)) => {
                map.get("channels").and_then(Value::as_array).map(|names| {
                    names
                        .iter()
                        .filter_map(|name| name.as_str().map(str::to_owned))
                        .collect::<HashSet<_>>()
                })
            }
            _ => None,
        };
        {
            let mut state = self.lock();
            state.init = InitState::Ready;
            state.channels = channels;
        }
        self.init_tx.send_replace(InitState::Ready);
    }

    fn send_request(
        &self,
        kind: i32,
        id: u32,
        channel: &str,
        method: Option<&str>,
        arg: Option<WireValue>,
    ) {
        let mut bytes = request_header(kind, Some(id), channel, method).encode();
        arg.unwrap_or(WireValue::Undefined).encode_into(&mut bytes);
        let _ = self.outbound.send(encode_regular(&bytes));
    }

    fn send_raw(&self, kind: i32, id: u32, arg: Option<WireValue>) {
        // TS `sendCancelOrDispose` 的 header 只有 `[type, id]`。
        let mut bytes =
            WireValue::Array(vec![WireValue::Int(kind), WireValue::Int(id as i32)]).encode();
        arg.unwrap_or(WireValue::Undefined).encode_into(&mut bytes);
        let _ = self.outbound.send(encode_regular(&bytes));
    }

    fn next_id(&self) -> u32 {
        self.last_request_id.fetch_add(1, Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ClientState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 请求头：`[type, id, channelName, methodName]`（cancel/dispose 只有 `[type, id, "", ""]` 形态的
/// 前两项由 TS 序列化，这里保持同一数组长度与末尾空串，避免服务端按位置读取时越界）。
fn request_header(kind: i32, id: Option<u32>, channel: &str, method: Option<&str>) -> WireValue {
    let mut items = vec![WireValue::Int(kind)];
    if let Some(id) = id {
        items.push(WireValue::Int(id as i32));
    }
    items.push(WireValue::String(channel.to_owned()));
    if let Some(method) = method {
        items.push(WireValue::String(method.to_owned()));
    }
    WireValue::Array(items)
}

#[cfg(test)]
#[path = "source_server_client_tests.rs"]
mod tests;
