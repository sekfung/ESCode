//! `source-server-management` 写面 RPC（docs/specs/source-server-remote-rust-host.md §9.8）。
//!
//! 参数布局与其它 `ProxyChannel.toService` 调用一致（实参打包成数组）。域校验错误回 `Error`
//! （与 Node 抛普通 Error 同义），结构/类型错误回 `InvalidRequest`。所有成功写都返回新快照。

use std::sync::Arc;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::credential::CredentialService;
use escode_host_services::source_server::{SourceServerRegistry, UpdateProfileInput};
use escode_host_services::source_server_connections::SourceServerConnections;
use serde_json::json;

use crate::channel::{MethodHandler, handler_rpc};
use crate::session::RpcError;
use crate::wire::Wire;

use super::{map_connect_error, snapshot_wire, string_arg};

pub(super) fn write_methods(
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("renameSource", rename_source(&registry, &connections)),
        ("renameProfile", rename_profile(&registry, &connections)),
        (
            "updateProfile",
            update_profile(&registry, &connections, credentials.clone()),
        ),
        (
            "setPreferredProfile",
            set_preferred_profile(&registry, &connections),
        ),
        (
            "removeProfile",
            remove_profile(&registry, &connections, credentials.clone()),
        ),
        (
            "removeSourceServer",
            remove_source_server(&registry, &connections, credentials),
        ),
        (
            "confirmIdentityChange",
            confirm_identity_change(&connections),
        ),
        (
            "ensureConversationWorkspace",
            ensure_conversation_workspace(&registry, &connections),
        ),
    ]
}

fn domain_error(error: anyhow::Error) -> RpcError {
    RpcError::new("Error", format!("{error:#}"))
}

fn rename_source(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "renameSource")?;
            let display_name = string_arg(&arg, 1, "renameSource")?;
            registry
                .rename_source(&source_server_id, &display_name)
                .await
                .map_err(domain_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

fn rename_profile(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let profile_id = string_arg(&arg, 0, "renameProfile")?;
            let display_name = string_arg(&arg, 1, "renameProfile")?;
            registry
                .rename_profile(&profile_id, &display_name)
                .await
                .map_err(domain_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

fn set_preferred_profile(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "setPreferredProfile")?;
            let profile_id = string_arg(&arg, 1, "setPreferredProfile")?;
            registry
                .set_preferred_profile(&source_server_id, &profile_id)
                .await
                .map_err(domain_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

fn update_profile(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        let credentials = credentials.as_ref().map(Arc::clone);
        Box::pin(async move {
            let payload = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .ok_or_else(|| {
                    RpcError::new(
                        "InvalidRequest",
                        "updateProfile requires an input object argument",
                    )
                })?;
            let input = UpdateProfileInput::from_json(&payload)
                .map_err(|error| RpcError::new("InvalidRequest", format!("{error:#}")))?;
            registry
                .update_profile(input, credentials.as_deref())
                .await
                .map_err(domain_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

fn remove_profile(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        let credentials = credentials.as_ref().map(Arc::clone);
        Box::pin(async move {
            let profile_id = string_arg(&arg, 0, "removeProfile")?;
            connections.disconnect(&profile_id).await;
            registry
                .remove_profile(&profile_id, credentials.as_deref())
                .await
                .map_err(domain_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

fn remove_source_server(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
    credentials: Option<Arc<CredentialService>>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        let credentials = credentials.as_ref().map(Arc::clone);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "removeSourceServer")?;
            connections.disconnect_source(&source_server_id).await;
            registry
                .remove_source_server(&source_server_id, credentials.as_deref())
                .await
                .map_err(domain_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

/// 用户确认后重连该 Profile；只有该入口允许 retire 旧 Profile 并绑定新身份（spec §11.12）。
fn confirm_identity_change(connections: &Arc<SourceServerConnections>) -> MethodHandler {
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let profile_id = string_arg(&arg, 0, "confirmIdentityChange")?;
            connections
                .connect_confirming_identity(&profile_id)
                .await
                .map_err(map_connect_error)?;
            snapshot_wire(&connections, false)
        })
    })
}

fn ensure_conversation_workspace(
    registry: &Arc<SourceServerRegistry>,
    connections: &Arc<SourceServerConnections>,
) -> MethodHandler {
    let registry = Arc::clone(registry);
    let connections = Arc::clone(connections);
    handler_rpc(move |arg: Wire| {
        let registry = Arc::clone(&registry);
        let connections = Arc::clone(&connections);
        Box::pin(async move {
            let source_server_id = string_arg(&arg, 0, "ensureConversationWorkspace")?;
            let client = connections
                .online_client(&source_server_id)
                .ok_or_else(|| {
                    RpcError::new(
                        "Error",
                        format!("Source Server must be connected: {source_server_id}"),
                    )
                })?;
            let value = client
                .call(
                    "project",
                    "prepareSession",
                    Some(WireValue::from(Wire::Array(Vec::new()))),
                )
                .await
                .map_err(|error| {
                    RpcError::new("Error", format!("project.prepareSession failed: {error:#}"))
                })?;
            let prepared = value.to_json().ok_or_else(|| {
                RpcError::new("Error", "project.prepareSession returned no object")
            })?;
            let path = prepared
                .get("workspacePath")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    RpcError::new("Error", "project.prepareSession returned no workspacePath")
                })?
                .to_owned();
            registry
                .record_conversation_workspace(&source_server_id, &path)
                .await
                .map_err(domain_error)?;
            Ok(Wire::Object(json!({
                "path": path,
                "created": true,
                "workspacePurpose": "conversation",
            })))
        })
    })
}
