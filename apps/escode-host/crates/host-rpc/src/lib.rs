//! Renderer RPC 服务面（rust-u2-host-skeleton.md §3.4）：`@escode/rpc` 协议跑在 WebSocket 上。
//!
//! 目标形态「Renderer（TS，不变）── WebSocket / RPC（capability 握手）──► Rust Host」的
//! Rust 侧实现：HTTP 路由与 capability 契约对齐 `packages/escode-server-cli/src/server-core/http.ts`，
//! 线协议对齐 `packages/rpc`（13 字节帧 + VSBuffer 序列化 + channel 分派）。
//! 未迁移的 channel 回 `NotImplemented`（Renderer 见显式缺口，不挂起）。

mod agent_v4;
mod channel;
mod collation;
mod connection;
mod deferred_drafts;
pub mod domains;
mod domains_attachments;
mod domains_broadcast;
mod domains_client;
mod domains_commands;
mod domains_credential;
pub mod domains_file;
mod domains_file_channel;
mod domains_file_index;
mod domains_file_preview;
mod domains_file_readdir;
mod domains_file_watcher;
mod domains_file_workspace;
mod domains_gen_ui;
mod domains_git;
mod domains_git_checkpoint;
mod domains_git_commit_message;
mod domains_hooks;
mod domains_mcp_sync;
mod domains_memory;
mod domains_onboarding;
mod domains_output_style;
mod domains_plugin;
mod domains_plugins_legacy;
mod domains_provider;
mod domains_provider_connectivity;
mod domains_provider_write;
mod domains_serial;
mod domains_session;
mod domains_session_drafts;
mod domains_session_model;
mod domains_session_write;
mod domains_setting_extra;
mod domains_settings_sync;
mod domains_simple;
mod domains_skill_sync;
mod domains_skills;
mod domains_subagents;
mod domains_system;
pub mod domains_task;
mod domains_task_archive;
mod domains_task_bot;
mod domains_task_bot_submit;
mod domains_task_bot_upload;
mod domains_task_commands;
mod domains_task_control;
mod domains_task_create;
mod domains_task_delete;
mod domains_task_extra;
mod domains_task_gap;
mod domains_task_goal;
mod domains_task_groups;
mod domains_task_import;
mod domains_task_input;
mod domains_task_lifecycle;
mod domains_task_query;
mod domains_task_runtime;
mod domains_task_send;
mod domains_task_snapshot;
mod domains_task_state;
mod domains_task_support;
mod domains_task_trajectory;
mod domains_terminal;
mod domains_usage_stats;
pub mod forward;
pub mod frame;
mod host_command;
pub mod http;
mod plugin_params;
mod plugin_result_schema;
mod plugin_result_tables;
mod project_memory;
pub mod runtime_requests;
pub mod session;
mod task_bot;
mod task_commands;
mod task_events;
mod task_index_syncer;
mod task_targets;
mod window_controller;
pub mod wire;
mod wire_bytes;
mod workspace_events;

use escode_host_control::logging;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};

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
    ///
    /// 修复（评审 A4）：旧令牌是「纳秒 ‖ pid」，可预测；改为 CSPRNG（两段 uuid v4，244 位随机）。
    pub fn issue(&self) -> String {
        let token = random_token();
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

/// CSPRNG 令牌（capability 与进程密钥共用）。
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// 常量时间比较（密钥校验不泄漏前缀长度）。
fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// 已启动的 RPC 服务面。
///
/// `secret` 是本次监听的进程密钥：只经 Main↔Host 控制通道交给 Main（`rpcListening.secret`），
/// 签发 capability 与读取 server-info 都要求带它（[`http::HOST_SECRET_HEADER`]）。
pub struct RpcSurface {
    pub addr: SocketAddr,
    pub secret: String,
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
    let secret: Arc<str> = Arc::from(random_token());
    let surface_secret = secret.to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                return;
            };
            let channels = Arc::clone(&channels);
            let capabilities = Arc::clone(&capabilities);
            let instances = instances.clone();
            let secret = Arc::clone(&secret);
            tokio::spawn(async move {
                let access = Access {
                    capabilities,
                    secret,
                };
                if let Err(error) = handle_connection(stream, channels, access, instances).await {
                    logging::error("rpc", format!("connection closed: {error:#}"));
                }
            });
        }
    });
    Ok(RpcSurface {
        addr,
        secret: surface_secret,
        task,
    })
}

/// 访问控制材料：一次性 capability 表 + 进程密钥。
struct Access {
    capabilities: Arc<Capabilities>,
    secret: Arc<str>,
}

impl Access {
    fn has_secret(&self, head: &http::RequestHead) -> bool {
        head.header(http::HOST_SECRET_HEADER)
            .is_some_and(|value| constant_time_eq(value, &self.secret))
    }
}

/// 请求头读取的总时限：半截请求头不能让连接任务无限轮询。
const HEAD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

async fn handle_connection(
    mut stream: TcpStream,
    channels: Arc<Channels>,
    access: Access,
    instances: Option<Arc<escode_host_runtime::Instances>>,
) -> Result<()> {
    // 先用 peek 判断是不是 WebSocket 升级：是则不消费字节，交给握手；
    // 否则按最小 HTTP 路由处理。
    let Some(head) = tokio::time::timeout(HEAD_DEADLINE, http::peek_head(&stream))
        .await
        .context("HTTP request head timed out")??
    else {
        return Ok(());
    };
    // 修复（评审 A9）：浏览器发起的请求一律带 `Origin`（跨源 WebSocket 不受同源策略约束，
    // DNS rebinding 也会带上）；Host 的合法调用方（Main 的 Node 客户端）从不带。直接拒绝，
    // 让网页无从触达可驱动 agent/工具的服务面。
    if head.header("origin").is_some() {
        let _ = http::read_head(&mut stream).await;
        return http::respond_json(
            &mut stream,
            403,
            "Forbidden",
            &json!({"error": "browser origins are not allowed"}),
        )
        .await;
    }
    if head.is_websocket_upgrade() {
        // 路径决定交付模式（TS server-core 同）：`/ws/host` = 桌面 continuous，`/ws` = 手机 replayable。
        let client_mode = match head.path.as_str() {
            "/ws/host" => session::ClientMode::DesktopContinuous,
            "/ws" => session::ClientMode::WebRemoteReplayable,
            _ => {
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
        };
        // 修复（评审 A9）：两条路径都要一次性 capability（旧实现 `/ws` 免鉴权，而 Main 正是把
        // 桌面 Renderer 桥到 `/ws`）。capability 校验在握手之前完成：失败回 HTTP 401 JSON。
        if !access
            .capabilities
            .consume(head.header(http::CAPABILITY_HEADER))
        {
            let _ = http::read_head(&mut stream).await;
            return http::respond_json(
                &mut stream,
                401,
                "Unauthorized",
                &json!({"error": "Invalid or expired host capability"}),
            )
            .await;
        }
        let ws = tokio_tungstenite::accept_async(stream)
            .await
            .context("WebSocket handshake failed")?;
        return connection::run(ws, channels, client_mode).await;
    }
    let Some(head) = http::read_head(&mut stream).await? else {
        return Ok(());
    };
    if !access.has_secret(&head) {
        return http::respond_json(
            &mut stream,
            401,
            "Unauthorized",
            &json!({"error": "Host secret required"}),
        )
        .await;
    }
    match (head.method.as_str(), head.path.as_str()) {
        ("GET", "/api/server-info") => {
            let info = json!({
                "serverId": "escode-host",
                "version": env!("CARGO_PKG_VERSION"),
                "protocolVersion": http::PROTOCOL_VERSION,
                "authRequired": true,
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
            let capability = access.capabilities.issue();
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
