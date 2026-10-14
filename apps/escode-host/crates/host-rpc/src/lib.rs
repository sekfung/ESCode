//! Renderer RPC 服务面（rust-u2-host-skeleton.md §3.4）：`@escode/rpc` 协议跑在 WebSocket 上。
//!
//! 目标形态「Renderer（TS，不变）── WebSocket / RPC（capability 握手）──► Rust Host」的
//! Rust 侧实现：HTTP 路由与 capability 契约对齐 `packages/escode-server-cli/src/server-core/http.ts`，
//! 线协议对齐 `packages/rpc`（13 字节帧 + VSBuffer 序列化 + channel 分派）。
//! 未迁移的 channel 回 `NotImplemented`（Renderer 见显式缺口，不挂起）。

pub mod domains;
pub mod forward;
pub mod frame;
pub mod http;
pub mod session;
pub mod wire;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

pub use session::Channels;

/// capability 表：一次性签发 + TTL（与 TS `createHostCapabilityStore` 语义一致的最小实现）。
#[derive(Default)]
pub struct Capabilities {
    tokens: std::sync::Mutex<std::collections::BTreeMap<String, std::time::Instant>>,
    ttl: std::time::Duration,
}

impl Capabilities {
    pub fn new(ttl: std::time::Duration) -> Self {
        Self {
            tokens: Default::default(),
            ttl,
        }
    }

    /// 签发一次性 capability。
    pub fn issue(&self) -> String {
        let token = format!(
            "{:x}{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
            std::process::id()
        );
        let mut tokens = self.lock();
        tokens.retain(|_, expires| *expires > std::time::Instant::now());
        tokens.insert(token.clone(), std::time::Instant::now() + self.ttl);
        token
    }

    /// 消费一次（成功即失效）。
    pub fn consume(&self, token: Option<&str>) -> bool {
        let Some(token) = token else {
            return false;
        };
        let mut tokens = self.lock();
        let now = std::time::Instant::now();
        tokens.retain(|_, expires| *expires > now);
        tokens.remove(token).is_some()
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::BTreeMap<String, std::time::Instant>> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 已启动的 RPC 服务面。
pub struct RpcSurface {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl RpcSurface {
    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// 在回环地址上监听（`127.0.0.1:0` 取临时端口）。
/// `instances` 仅供 `/api/server-info` 报告已建实例（域内路由由 channel 自己持有）。
pub async fn serve(
    bind: SocketAddr,
    channels: Channels,
    instances: Option<escode_host_runtime::Instances>,
) -> Result<RpcSurface> {
    http::ensure_loopback(bind)?;
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("Host RPC surface cannot bind {bind}"))?;
    let addr = listener.local_addr()?;
    let channels = Arc::new(channels);
    let capabilities = Arc::new(Capabilities::new(std::time::Duration::from_secs(60)));
    let instances = instances.map(Arc::new);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                return;
            };
            let channels = Arc::clone(&channels);
            let capabilities = Arc::clone(&capabilities);
            let instances = instances.clone();
            tokio::spawn(async move {
                if let Err(error) =
                    handle_connection(stream, channels, capabilities, instances).await
                {
                    eprintln!("escode-host: rpc connection closed: {error:#}");
                }
            });
        }
    });
    Ok(RpcSurface { addr, task })
}

async fn handle_connection(
    mut stream: TcpStream,
    channels: Arc<Channels>,
    capabilities: Arc<Capabilities>,
    instances: Option<Arc<escode_host_runtime::Instances>>,
) -> Result<()> {
    // 先用 peek 判断是不是 WebSocket 升级：是则不消费字节，交给握手；
    // 否则按最小 HTTP 路由处理。
    let Some(head) = http::peek_head(&stream).await? else {
        return Ok(());
    };
    if head.is_websocket_upgrade() {
        // capability 校验在握手之前完成：失败回 HTTP 401 JSON（与 TS `context.json(..., 401)` 同形），
        // 不留裸连接重置。
        if head.path != "/ws" && head.path != "/ws/host" {
            // 拒绝前先把请求头读掉：带未读数据关闭连接会触发 RST，客户端看不到 404/401。
            let _ = http::read_head(&mut stream).await;
            return http::respond_json(
                &mut stream,
                404,
                "Not Found",
                &json!({"error": "unknown WebSocket path"}),
            )
            .await;
        }
        if head.path == "/ws/host" {
            let token = head.header(http::CAPABILITY_HEADER);
            if !capabilities.consume(token) {
                let _ = http::read_head(&mut stream).await;
                return http::respond_json(
                    &mut stream,
                    401,
                    "Unauthorized",
                    &json!({"error": "Invalid or expired host capability"}),
                )
                .await;
            }
        }
        return session_over_websocket(stream, channels).await;
    }
    let Some(head) = http::read_head(&mut stream).await? else {
        return Ok(());
    };
    match (head.method.as_str(), head.path.as_str()) {
        ("GET", "/api/server-info") => {
            let info = json!({
                "serverId": "escode-host",
                "version": env!("CARGO_PKG_VERSION"),
                "protocolVersion": http::PROTOCOL_VERSION,
                "authRequired": false,
                "workspaces": instances
                    .as_ref()
                    .map(|instances| instances.identities())
                    .unwrap_or_default(),
                "capabilities": {
                    "desktopContinuous": true,
                    "websocketRpc": true,
                    "processResourceTelemetry": false,
                },
                "channels": channels.names(),
            });
            http::respond_json(&mut stream, 200, "OK", &info).await
        }
        ("POST", "/api/rpc-host-capability") => {
            let capability = capabilities.issue();
            http::respond_json(
                &mut stream,
                200,
                "OK",
                &json!({"capability": capability, "expiresInMs": 60_000}),
            )
            .await
        }
        _ => {
            http::respond_json(
                &mut stream,
                404,
                "Not Found",
                &json!({"error": "unknown Host route"}),
            )
            .await
        }
    }
}

/// WebSocket 握手 + RPC 会话（鉴权已在调用方完成）。
async fn session_over_websocket(stream: TcpStream, channels: Arc<Channels>) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .context("WebSocket handshake failed")?;
    session(ws, channels).await
}

async fn session(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    channels: Arc<Channels>,
) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};

    let (mut sink, mut source) = ws.split();
    // 连接建立即 Initialize（TS ChannelServer 构造时发送）。
    sink.send(Message::Binary(
        frame::frame(&session::initialize_frame()).into(),
    ))
    .await
    .context("write initialize failed")?;

    let mut frames = frame::FrameReader::new();
    while let Some(message) = source.next().await {
        match message {
            Ok(Message::Binary(bytes)) => frames.push(&bytes),
            Ok(Message::Text(text)) => frames.push(text.as_bytes()),
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        }
        while let Some(body) = frames.next_frame()? {
            let request = match session::parse_request(&body) {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("escode-host: invalid RPC frame: {error:#}");
                    continue;
                }
            };
            let response = session::respond(&channels, &request).await;
            if response.is_empty() {
                continue;
            }
            sink.send(Message::Binary(frame::frame(&response).into()))
                .await
                .context("write RPC response failed")?;
        }
    }
    Ok(())
}
