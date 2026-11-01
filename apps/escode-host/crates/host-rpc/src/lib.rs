//! Renderer RPC 服务面（rust-u2-host-skeleton.md §3.4）：`@escode/rpc` 协议跑在 WebSocket 上。
//!
//! 目标形态「Renderer（TS，不变）── WebSocket / RPC（capability 握手）──► Rust Host」的
//! Rust 侧实现：HTTP 路由与 capability 契约对齐 `packages/escode-server-cli/src/server-core/http.ts`，
//! 线协议对齐 `packages/rpc`（13 字节帧 + VSBuffer 序列化 + channel 分派）。
//! 未迁移的 channel 回 `NotImplemented`（Renderer 见显式缺口，不挂起）。

mod agent_v4;
mod automation_dispatch;
mod automation_scheduler;
mod capability;
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
mod domains_cua_permission;
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
mod domains_plugin_sync;
mod domains_plugin_ui;
mod domains_plugin_ui_app_tools;
mod domains_plugin_ui_sampling;
mod domains_plugins_legacy;
mod domains_project;
mod domains_project_remote;
mod domains_project_session;
mod domains_provider;
mod domains_provider_connectivity;
mod domains_provider_write;
mod domains_remote_projects;
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
mod domains_source_server;
mod domains_subagents;
mod domains_svn;
mod domains_svn_commit_message;
mod domains_system;
pub mod domains_task;
mod domains_task_archive;
mod domains_task_commands;
mod domains_task_control;
mod domains_task_create;
mod domains_task_delete;
mod domains_task_extra;
mod domains_task_gap;
mod domains_task_goal;
mod domains_task_groups;
mod domains_task_groups_service;
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
mod domains_workspace_vcs;
mod event_fanout;
pub mod forward;
pub mod frame;
mod host_command;
pub mod host_events;
pub mod http;
mod plugin_params;
mod plugin_result_schema;
mod plugin_result_tables;
mod plugin_ui_meta;
mod plugin_ui_prepare;
mod project_archive;
mod project_memory;
mod provider_headers;
mod remote_host_id;
mod remote_scope;
mod runtime_event_fanout;
pub mod runtime_requests;
pub mod session;
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

pub use capability::{Capabilities, CapabilityGrant};
pub use domains_source_server::{
    HostEnvironment, remote_profile_resolver, remote_scope_channel_set, remote_scope_ensurer,
};
pub use remote_scope::{
    REMOTE_SCOPE_HOST_CHANNELS, RemoteGrant, RemoteProfileCanceller, RemoteProfileRequest,
    RemoteProfileResolveError, RemoteProfileResolver, RemoteScope, RemoteScopeEnsurer,
    ResolvedRemoteProfile,
};
pub use session::Channels;

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
    let secret: Arc<str> = Arc::from(capability::random_token());
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
        // 路径决定交付模式（TS server-core 同）：`/ws/host` = 桌面 continuous，`/ws` = 远程 workspace replayable。
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
        let Some(grant) = access
            .capabilities
            .consume(head.header(http::CAPABILITY_HEADER))
        else {
            let _ = http::read_head(&mut stream).await;
            return http::respond_json(
                &mut stream,
                401,
                "Unauthorized",
                &json!({"error": "Invalid or expired host capability"}),
            )
            .await;
        };
        let ws = tokio_tungstenite::accept_async(stream)
            .await
            .context("WebSocket handshake failed")?;
        return connection::run(ws, channels, client_mode, grant).await;
    }
    // 读取头与 body：`read_head` 会把同一 chunk 里跟进的 body 一起消费掉（capability 的 scope 要读 body）。
    let Some((head, body)) = http::read_request(&mut stream).await? else {
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
                },
                "channels": channels.names(),
            });
            http::respond_json(&mut stream, 200, "OK", &info).await
        }
        ("POST", "/api/rpc-host-capability") => {
            // 远端 scope 先建连接再签发；失败是显式 4xx，不下发本地权限的令牌（spec §10.11）。
            match capability::issue_for(&channels, &access.capabilities, &body).await {
                Ok(capability) => {
                    http::respond_json(
                        &mut stream,
                        200,
                        "OK",
                        &json!({"capability": capability, "expiresInMs": 60_000}),
                    )
                    .await
                }
                Err(error) => {
                    http::respond_json(&mut stream, 400, "Bad Request", &json!({"error": error}))
                        .await
                }
            }
        }
        ("POST", "/api/source-server-resolve-profile") => {
            match channels.resolve_remote_profile(&body).await {
                Ok(profile) => {
                    escode_host_control::logging::info(
                        "source-server",
                        format!(
                            "resolved first-connect profile {} for source {}",
                            profile.profile_id, profile.source_server_id
                        ),
                    );
                    http::respond_json(
                        &mut stream,
                        200,
                        "OK",
                        &json!({
                            "profileId": profile.profile_id,
                            "sourceServerId": profile.source_server_id,
                            "target": profile.target,
                        }),
                    )
                    .await
                }
                Err(error) => {
                    let status = error.status_code();
                    let reason = if status == 409 {
                        "Conflict"
                    } else {
                        "Bad Request"
                    };
                    http::respond_json(&mut stream, status, reason, &error.to_json()).await
                }
            }
        }
        ("POST", "/api/source-server-cancel-profile") => {
            // 取消只向对应 caller 的 watch 置位；未知/已完成 key 返回 false（幂等）。
            match channels.cancel_remote_profile(&body) {
                Ok(cancelled) => {
                    http::respond_json(&mut stream, 200, "OK", &json!({"cancelled": cancelled}))
                        .await
                }
                Err(error) => {
                    http::respond_json(&mut stream, 400, "Bad Request", &error.to_json()).await
                }
            }
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
