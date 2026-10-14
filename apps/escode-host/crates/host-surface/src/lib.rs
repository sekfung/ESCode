//! Renderer 传输面：WebSocket + capability 握手（rust-u2-host-skeleton.md §3.4）。
//!
//! 目标形态是「Renderer（TS，不变）── WebSocket / RPC（capability 握手）──► Rust Host」。
//! 本 crate 提供 Rust 侧监听与连接处理，**帧面沿用既有 V4 App 协议**（与 stdio 相同的
//! NDJSON 行），Renderer 的协议客户端可以原样复用；只有握手信封是本 crate 的最小增量
//! （`Hello` → `Ready`，登记在 u2 子 spec）。
//!
//! 规则（与 host-gateway 一致）：
//! 1. 连接按**到达序**登记（`Gateway::register` 同步取号），每连接一个写者；
//! 2. capability 握手求交：未覆盖的 method 立即回 `NotImplemented`（缺口显式可见）；
//! 3. 一个 workspace 实例只有一个帧泵，向全部 attachment 扇出（多窗口共享实例）。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use escode_cli_rust::embed::RuntimeOptions;

use anyhow::{Context, Result};
use escode_host_control::WorkspaceDescriptor;
use escode_host_gateway::{Capabilities, ConnectionId, Gateway, Route};
use escode_host_runtime::{Instance, Instances};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;

/// 连接出站队列容量（与 Gateway 出站策略一致：满即丢可重建流，见 u2 子 spec §3.2）。
const OUTBOUND_CAPACITY: usize = 256;

/// 握手：Renderer 连接后的第一帧。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub window_id: String,
    pub client_id: String,
    pub workspace: WorkspaceDescriptor,
    /// 客户端声明支持的 method；与 Host 支持面求交后生效。
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// 握手应答。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ready {
    pub r#type: &'static str,
    pub generation: u64,
    /// 求交后的能力集合（空 = 只能收错误帧）。
    pub capabilities: Vec<String>,
}

impl Ready {
    fn json(generation: u64, capabilities: Vec<String>) -> String {
        serde_json::to_string(&Self {
            r#type: "ready",
            generation,
            capabilities,
        })
        .unwrap_or_default()
    }
}

/// 错误帧（与 V4 的 rpc_error 同形，Renderer 可复用同一错误处理）。
pub fn error_frame(id: Option<&str>, code: i64, message: &str) -> String {
    serde_json::json!({
        "id": id.unwrap_or("host-surface"),
        "error": {"code": code, "message": message},
    })
    .to_string()
}

/// 已启动的传输面。
pub struct Surface {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Surface {
    /// 关闭监听（在飞连接由各自任务收口）。
    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// RuntimeOptions 的构造注入点：组合根（Host 进程 / 测试）提供，crate 内不读进程 env
/// （边界脚本规则：Host 的 env 来自控制消息或显式注入）。
pub type OptionsFactory = Arc<dyn Fn(&WorkspaceDescriptor) -> RuntimeOptions + Send + Sync>;

/// 在 `bind` 上监听（`127.0.0.1:0` 取临时端口，供测试与 Main 选择端口）。
pub async fn serve(
    bind: SocketAddr,
    instances: Instances,
    options: OptionsFactory,
) -> Result<Surface> {
    Surface::start(bind, instances, options).await
}

impl Surface {
    /// 启动监听（`serve` 的别名，测试与 Main 通用）。
    pub async fn start(
        bind: SocketAddr,
        instances: Instances,
        options: OptionsFactory,
    ) -> Result<Self> {
        let listener = TcpListener::bind(bind)
            .await
            .with_context(|| format!("Host surface cannot bind {bind}"))?;
        let addr = listener.local_addr()?;
        let hub = Arc::new(Hub::new(instances, options));
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else {
                    return;
                };
                let hub = Arc::clone(&hub);
                tokio::spawn(async move {
                    if let Err(error) = hub.serve_connection(stream).await {
                        eprintln!("escode-host: surface connection {peer} closed: {error:#}");
                    }
                });
            }
        });
        Ok(Self { addr, task })
    }
}

/// 连接集合与按实例的帧泵。
struct Hub {
    instances: Instances,
    gateway: Gateway,
    pumps: Mutex<BTreeMap<String, Pump>>,
    options: OptionsFactory,
}

#[derive(Default)]
struct Pump {
    /// 该实例的 attachment（connection → 出站队列发送端）。
    connections: BTreeMap<ConnectionId, mpsc::Sender<String>>,
    /// 帧泵是否已启动（一个实例一个，扇出到全部 attachment）。
    started: bool,
}

impl Hub {
    fn new(instances: Instances, options: OptionsFactory) -> Self {
        Self {
            instances,
            gateway: Gateway::new(),
            pumps: Mutex::new(BTreeMap::new()),
            options,
        }
    }

    async fn serve_connection(self: Arc<Self>, stream: TcpStream) -> Result<()> {
        let ws = tokio_tungstenite::accept_async(stream)
            .await
            .context("WebSocket handshake failed")?;
        let (sink, source) = ws.split();

        // 每连接一个写者：出站帧按到达序写出（tx 归本连接任务与实例帧泵共享）。
        let (tx, rx) = mpsc::channel::<String>(OUTBOUND_CAPACITY);
        let writer = tokio::spawn(async move {
            let mut sink = sink;
            let mut rx = rx;
            while let Some(line) = rx.recv().await {
                if sink.send(Message::Text(line.into())).await.is_err() {
                    return;
                }
            }
        });

        let outcome = Arc::clone(&self).session(source, tx).await;
        if let Ok(connection) = &outcome {
            // 收口：注销连接（attachment 已在 session 内摘除；实例保持热态）。
            self.gateway.unregister(*connection);
        }
        writer.abort();
        outcome.map(|_| ())
    }

    /// 握手 + 请求循环；返回连接结束原因。
    async fn session(
        self: Arc<Self>,
        mut source: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<TcpStream>,
        >,
        tx: mpsc::Sender<String>,
    ) -> Result<ConnectionId> {
        let hello_line = read_text_frame(&mut source).await?;
        let hello: Hello = serde_json::from_str(&hello_line).context("Invalid hello frame")?;
        let capabilities = Capabilities::negotiate(&hello.capabilities, SUPPORTED_METHODS);
        let (lane, _gateway_outbound) = self
            .gateway
            .register(capabilities.clone(), OUTBOUND_CAPACITY);
        let connection = lane.id;

        let identity = identity_of(&hello.workspace);
        let options = Arc::clone(&self.options);
        let instance = match self
            .instances
            .get(&identity, || options(&hello.workspace))
            .await
        {
            Ok(instance) => instance,
            Err(error) => {
                let code = if error
                    .downcast_ref::<escode_cli_rust::adapters::storage::WorkspaceOwned>()
                    .is_some()
                {
                    "workspace_owned"
                } else {
                    "fatal"
                };
                let frame = serde_json::json!({
                    "type": "error",
                    "error": {"code": code, "message": format!("{error:#}")},
                })
                .to_string();
                let _ = tx.send(frame).await;
                return Ok(connection);
            }
        };

        let ready = Ready::json(
            instance.generation,
            capabilities
                .methods()
                .iter()
                .map(|m| (*m).to_owned())
                .collect(),
        );
        let _ = tx.send(ready).await;
        attach(&self, &identity, connection, tx.clone(), instance.clone()).await;

        while let Some(message) = source.next().await {
            let text = match message {
                Ok(Message::Text(text)) => text.to_string(),
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            let parsed = serde_json::from_str::<serde_json::Value>(&text).ok();
            let method = parsed.as_ref().and_then(|v| v["method"].as_str());
            let id = parsed.as_ref().and_then(|v| v["id"].as_str());
            match method.map(|method| self.gateway.route(connection, method)) {
                Some(Route::NotImplemented(method)) => {
                    let frame = error_frame(
                        id,
                        -32601,
                        &format!("Host does not implement `{method}` yet"),
                    );
                    let _ = tx.send(frame).await;
                }
                // 覆盖的请求与非请求帧（Notification 等）都原样转给实例：协议面一致。
                _ => {
                    if let Err(error) = instance.send_line(&text).await {
                        let frame = error_frame(id, -32603, &format!("{error:#}"));
                        let _ = tx.send(frame).await;
                    }
                }
            }
        }
        self.detach(&identity, connection).await;
        Ok(connection)
    }

    async fn detach(&self, identity: &str, connection: ConnectionId) {
        let mut pumps = self.pumps.lock().await;
        if let Some(pump) = pumps.get_mut(identity) {
            pump.connections.remove(&connection);
        }
    }
}

/// 把连接登记到实例的扇出集合；首个 attachment 启动该实例的帧泵。
async fn attach(
    hub: &Arc<Hub>,
    identity: &str,
    connection: ConnectionId,
    outbound: mpsc::Sender<String>,
    instance: Arc<Instance>,
) {
    let start_pump = {
        let mut pumps = hub.pumps.lock().await;
        let pump = pumps.entry(identity.to_owned()).or_default();
        pump.connections.insert(connection, outbound);
        if pump.started {
            false
        } else {
            pump.started = true;
            true
        }
    };
    if !start_pump {
        return;
    }
    // 首个 attachment：为该实例启动帧泵（扇出到当前与后续连接）。泵随实例结束而退出，
    // 并清掉自己的条目——实例换代后下一次 attach 会重启泵。
    let hub = Arc::clone(hub);
    let identity = identity.to_owned();
    tokio::spawn(async move {
        while let Some(line) = instance.line().await {
            let targets: Vec<mpsc::Sender<String>> = {
                let pumps = hub.pumps.lock().await;
                pumps
                    .get(&identity)
                    .map(|pump| pump.connections.values().cloned().collect())
                    .unwrap_or_default()
            };
            for target in targets {
                // 队列满（慢连接）时跳过该连接：可重建流由快照恢复补齐（§3.2 策略）。
                let _ = target.try_send(line.clone());
            }
        }
        hub.pumps.lock().await.remove(&identity);
    });
}

/// 读一条文本帧（跳过 Ping/Pong/Binary；Close/EOF 结束）。
async fn read_text_frame<S>(source: &mut S) -> Result<String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match source.next().await {
            Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
            Some(Ok(Message::Close(_))) | None => anyhow::bail!("connection closed before hello"),
            Some(Ok(_)) => continue,
            Some(Err(error)) => return Err(error).context("WebSocket read failed"),
        }
    }
}

/// Host 支持的 method 集（capability 求交用；与 `host-proto` 清单同源，随域迁移扩展）。
const SUPPORTED_METHODS: &[&str] = &[
    "capabilities",
    "session/list",
    "v4/command",
    "v4/conversation/subscribe",
    "v4/conversation/resync",
    "v4/conversation/unsubscribe",
    "settings/read",
    "settings/write",
];

pub fn identity_of(workspace: &WorkspaceDescriptor) -> String {
    workspace
        .identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| workspace.path.clone())
}
