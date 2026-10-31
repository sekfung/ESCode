//! `source-server-management`（S2–S4，docs/specs/source-server-remote-rust-host.md §9–§11）。
//!
//! `getSnapshot` / `getRecentOperationLogs` / 连接与断开、三个事件订阅在本模块；
//! rename / remove / update / 迁移报告等写面在 [`write_handlers`] / [`migration_handlers`]，
//! 统一走 registry 写锁与 `settings::apply_patch`。连接状态由 `SourceServerConnections`
//! 写入 registry，事件流只在状态真正变化时发射。生产 transport/凭据/probe 装配见 [`options`]。

use std::sync::Arc;

use escode_host_services::credential::CredentialService;
use escode_host_services::source_server::{
    IdentityChangePending, SourceServerEvent, SourceServerRegistry,
};
use escode_host_services::source_server_connections::SourceServerConnections;
use escode_host_services::source_server_update::UpdateRequest;
use serde_json::json;

use crate::channel::{Channel, EventHandler, EventStream, handler_rpc};
use crate::session::RpcError;
use crate::wire::Wire;

#[path = "domains_source_server_migration.rs"]
mod migration_handlers;
#[path = "domains_source_server_options.rs"]
mod options;
#[path = "domains_source_server_writes.rs"]
mod write_handlers;

pub use options::{HostEnvironment, default_connections_options};

/// S4-e 组合根：启动 `lastOnlineAt` 串行落盘、登记自动恢复 manager，并异步发起
/// 启动自动连接（≤3 并发、每 Source 一次）。连接失败只落状态，不影响 RPC 装配。
pub fn start_source_server_recovery(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
) {
    let _ = registry.start_last_online_writer();
    SourceServerConnections::install_self(connections);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64);
    let candidates = registry.startup_candidates(
        now_ms,
        escode_host_services::source_server_recovery::STARTUP_RECONNECT_WINDOW_MS,
    );
    if candidates.is_empty() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let manager = Arc::clone(connections);
    handle.spawn(async move {
        manager.startup_reconnect(candidates).await;
    });
}

pub fn source_server_management_channel(
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> Channel {
    let snapshot_connections = Arc::clone(&connections);
    let snapshot = handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&snapshot_connections);
        Box::pin(async move {
            let refresh_migration_counts = migration_handlers::parse_snapshot_request(&arg)?;
            let snapshot = connections
                .snapshot(refresh_migration_counts)
                .map_err(|error| RpcError::new("Fatal", error))?;
            Ok(Wire::Object(snapshot))
        })
    });

    let logs_registry = Arc::clone(&registry);
    let logs = handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&logs_registry);
        Box::pin(async move {
            let profile_id = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| {
                    RpcError::new(
                        "InvalidRequest",
                        "getRecentOperationLogs requires a profile id string",
                    )
                })?;
            let logs = registry
                .recent_operation_logs(&profile_id)
                .into_iter()
                .map(Wire::Object)
                .collect();
            Ok(Wire::Array(logs))
        })
    });

    let connect_connections = Arc::clone(&connections);
    let connect = handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&connect_connections);
        Box::pin(async move {
            let profile_id = string_arg(&arg, 0, "connectProfile")?;
            // `retainedOnly` 只是恢复入口的过滤提示（当前无调用方），连接语义不变（spec §10.10）。
            connections
                .connect(&profile_id)
                .await
                .map_err(map_connect_error)?;
            snapshot_wire(&connections, false)
        })
    });

    let disconnect_connections = Arc::clone(&connections);
    let disconnect = handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&disconnect_connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "disconnectSource")?;
            connections.disconnect_source(&source_server_id).await;
            snapshot_wire(&connections, false)
        })
    });

    let update_connections = Arc::clone(&connections);
    let update = handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&update_connections);
        Box::pin(async move {
            let payload = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .ok_or_else(|| {
                    RpcError::new(
                        "InvalidRequest",
                        "updateSource requires a request object argument",
                    )
                })?;
            let request = UpdateRequest::from_json(&payload)
                .map_err(|error| RpcError::new("InvalidRequest", format!("{error:#}")))?;
            connections.update_source(request).await.map_err(|error| {
                let mut rpc = RpcError::new("Error", error.message);
                if let Some(code) = error.code {
                    rpc = rpc.with("code", json!(code));
                }
                if let Some(retryable) = error.retryable {
                    rpc = rpc.with("retryable", json!(retryable));
                }
                rpc
            })?;
            snapshot_wire(&connections, false)
        })
    });

    let state_events = state_event_handler(Arc::clone(&registry));
    let directory_events = directory_event_handler(Arc::clone(&registry));
    let operation_log_events = operation_log_event_handler(Arc::clone(&registry));
    let mut methods = vec![
        ("getSnapshot", snapshot),
        ("getRecentOperationLogs", logs),
        ("connectProfile", connect),
        ("disconnectSource", disconnect),
        ("updateSource", update),
    ];
    methods.extend(migration_handlers::migration_methods(
        Arc::clone(&registry),
        Arc::clone(&connections),
    ));
    methods.extend(write_handlers::write_methods(
        Arc::clone(&registry),
        Arc::clone(&connections),
        credentials,
    ));
    Channel::new("source-server-management", methods).with_events(vec![
        ("onDidChangeState", state_events),
        ("onDidChangeDirectory", directory_events),
        ("onDidAppendOperationLog", operation_log_events),
    ])
}

fn snapshot_wire(
    connections: &Arc<SourceServerConnections>,
    refresh: bool,
) -> Result<Wire, RpcError> {
    connections
        .snapshot(refresh)
        .map(Wire::Object)
        .map_err(|error| RpcError::new("Fatal", error))
}

/// 位置参数必须是字符串（`ProxyChannel.toService` 把实参打包成数组）。
fn string_arg(arg: &Wire, index: usize, method: &str) -> Result<String, RpcError> {
    arg.clone()
        .into_args()
        .into_iter()
        .nth(index)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| {
            RpcError::new(
                "InvalidRequest",
                format!("{method} requires a string argument at index {index}"),
            )
        })
}

/// `connectProfile` / `confirmIdentityChange` 的错误映射：身份变化必须带结构化 code。
pub(super) fn map_connect_error(error: anyhow::Error) -> RpcError {
    if let Some(pending) = error.downcast_ref::<IdentityChangePending>() {
        return RpcError::new("Error", pending.to_string())
            .with("code", json!("source-server-identity-change-pending"))
            .with("profileId", json!(pending.profile_id))
            .with(
                "pendingSourceServerId",
                json!(pending.pending_source_server_id),
            )
            .with("retryable", json!(false));
    }
    RpcError::new("Error", format!("{error:#}"))
}

/// 只转发本订阅者关心的事件类型；registry 关闭（Sender 丢弃）时流结束。
fn event_stream<F>(registry: &SourceServerRegistry, map: F) -> EventStream
where
    F: Fn(SourceServerEvent) -> Option<Wire> + Send + 'static,
{
    let receiver = registry.subscribe();
    Box::pin(futures_util::stream::unfold(
        (receiver, map),
        |(mut receiver, map)| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if let Some(wire) = map(event) {
                            return Some((wire, (receiver, map)));
                        }
                    }
                    Err(_) => return None,
                }
            }
        },
    ))
}

fn state_event_handler(registry: Arc<SourceServerRegistry>) -> EventHandler {
    Arc::new(move |_arg: Wire| {
        Ok(event_stream(&registry, |event| match event {
            SourceServerEvent::State(state) => Some(Wire::Object(state)),
            _ => None,
        }))
    })
}

fn directory_event_handler(registry: Arc<SourceServerRegistry>) -> EventHandler {
    Arc::new(move |_arg: Wire| {
        Ok(event_stream(&registry, |event| match event {
            SourceServerEvent::Directory(generation) => {
                Some(Wire::Object(json!({ "generation": generation })))
            }
            _ => None,
        }))
    })
}

fn operation_log_event_handler(registry: Arc<SourceServerRegistry>) -> EventHandler {
    Arc::new(move |_arg: Wire| {
        Ok(event_stream(&registry, |event| match event {
            SourceServerEvent::OperationLog(log) => Some(Wire::Object(log)),
            _ => None,
        }))
    })
}

/// 远端 scope 的 ensure：复用 S3-a 连接管理器（per-profile single-flight，`Initialize` 完成后返回）。
///
/// capability 签发方在拿到客户端后才下发令牌；这里把连接/握手错误转成可读原因（签发失败即 4xx）。
pub fn remote_scope_ensurer(
    connections: Arc<SourceServerConnections>,
) -> crate::RemoteScopeEnsurer {
    type RemoteFuture = std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Arc<escode_host_services::source_server_client::RemoteChannelClient>,
                        String,
                    >,
                > + Send,
        >,
    >;
    Arc::new(move |scope: crate::RemoteScope| -> RemoteFuture {
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            connections
                .connect(&scope.profile_id)
                .await
                .map_err(|error| format!("{error:#}"))?;
            connections.client(&scope.profile_id).ok_or_else(|| {
                format!(
                    "source server profile {} has no client after connect",
                    scope.profile_id
                )
            })
        })
    })
}

/// 首连取消（S5-a cancel）：只向 caller 的 watch 置位，不按 Profile 无差别 abort。
pub fn remote_profile_canceller(
    connections: Arc<SourceServerConnections>,
) -> crate::RemoteProfileCanceller {
    Arc::new(move |caller_key: &str| connections.cancel_cancellable(caller_key))
}

/// S5-a 远端 scope 装配：ensure 与首连 resolver 共用同一连接管理器。
pub struct RemoteScopeSupport {
    pub ensurer: crate::RemoteScopeEnsurer,
    pub profile_resolver: crate::RemoteProfileResolver,
    pub profile_canceller: crate::RemoteProfileCanceller,
}

pub fn remote_scope_support(
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> RemoteScopeSupport {
    RemoteScopeSupport {
        ensurer: remote_scope_ensurer(Arc::clone(&connections)),
        profile_resolver: remote_profile_resolver(registry, Arc::clone(&connections), credentials),
        profile_canceller: remote_profile_canceller(connections),
    }
}

/// 组合根一次性登记 Source 管理面 channel 与远端 scope 能力（顺序：先取 handles 再交管理面）。
pub fn attach_source_server_channels(
    list: &mut Vec<Channel>,
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> RemoteScopeSupport {
    let remote_scope = remote_scope_support(
        Arc::clone(&registry),
        Arc::clone(&connections),
        credentials.clone(),
    );
    list.push(source_server_management_channel(
        registry,
        connections,
        credentials,
    ));
    remote_scope
}

/// 首连 Profile 解析（S5-a）：ensure Profile → 显式连接 → 返回握手回填后的 Source identity。
///
/// Main 只提交脱敏目标（SSH 密码仅用于本次 Host 内部落凭据）；settings 仍由 registry 唯一写入。
pub fn remote_profile_resolver(
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> crate::RemoteProfileResolver {
    type ProfileFuture = std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::ResolvedRemoteProfile, crate::RemoteProfileResolveError>,
                > + Send,
        >,
    >;
    Arc::new(move |request| -> ProfileFuture {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        let credentials = credentials.clone();
        Box::pin(async move {
            // 取消按 caller key 作用域登记：同一 Profile 的其它窗口不受影响（spec §12.2）。
            let caller_key = request.request_id.clone();
            let cancel = caller_key
                .as_deref()
                .map(|key| connections.register_cancellable(key));
            let profile_id = match request.profile_id {
                Some(profile_id) => {
                    registry.profile(&profile_id).ok_or_else(|| {
                        crate::RemoteProfileResolveError::new(format!(
                            "unknown source server profile: {profile_id}"
                        ))
                    })?;
                    profile_id
                }
                None => {
                    let target = request.target.ok_or_else(|| {
                        crate::RemoteProfileResolveError::new(
                            "source server profile resolution requires target",
                        )
                    })?;
                    registry
                        .ensure_profile(&target, credentials.as_deref())
                        .await
                        .map_err(crate::RemoteProfileResolveError::from_anyhow)?
                        .profile_id
                }
            };
            let connected = match cancel {
                Some(cancel) => connections.connect_cancellable(&profile_id, cancel).await,
                None => connections.connect(&profile_id).await,
            };
            if let Some(key) = caller_key.as_deref() {
                connections.finish_cancellable(key);
            }
            connected.map_err(crate::RemoteProfileResolveError::from_anyhow)?;
            let profile = registry.profile(&profile_id).ok_or_else(|| {
                crate::RemoteProfileResolveError::new(format!(
                    "source server profile disappeared during connect: {profile_id}"
                ))
            })?;
            let source_server_id = profile.source_server_id.clone().ok_or_else(|| {
                crate::RemoteProfileResolveError::new(format!(
                    "source server profile {profile_id} has no sourceServerId after connect"
                ))
            })?;
            Ok(crate::ResolvedRemoteProfile {
                profile_id,
                source_server_id,
                target: profile.target,
            })
        })
    })
}

/// 远端 scope 的连接级 channel 集合：只有绑定远端授权的连接才产出转发入口。
///
/// 本地连接返回空（`Channels::connect` 不会为本地连接调用本工厂）。
pub fn remote_scope_channel_set() -> crate::session::ChannelSetFactory {
    Arc::new(|context: &crate::session::ConnectionContext| {
        let Some(grant) = context.remote.as_ref() else {
            return Vec::new();
        };
        let mut channels = crate::remote_scope::forwarded_channels(Arc::clone(&grant.client));
        // 每条远端连接用自己的投影服务 window-controller（spec §10.13）；同名共享 channel 被连接级实现取代。
        channels.push(crate::window_controller::remote_channel(
            Arc::clone(&grant.client),
            &grant.scope,
        ));
        channels
    })
}

/// 远端 server 启动脚本（与 TS `buildRemoteServerCommand` 对齐）。
///
/// 由 transport 包成 `sh -lc <script>`，统一 SSH / WSL / Docker 的引号语义；`~`/`$HOME`
/// 由远端展开。安装与探测复用同一 transport（`RemoteExecutor`），但只有显式连接才执行。
const DEFAULT_REMOTE_SERVER_SCRIPT: &str = "ESCODE_SERVICE_AUTHORITY_MODE=\"desktop-attached-remote\" ESCODE_SERVER_RUNTIME_ROOT=\"$HOME/.escode/server\" ~/.escode/server/node ~/.escode/server/zcode-server.cjs";

#[cfg(test)]
#[path = "domains_source_server_tests.rs"]
mod tests;
