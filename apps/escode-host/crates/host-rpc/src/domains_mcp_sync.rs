//! `mcp-sync` channel（TS `ServiceChannels.McpSync` = `"mcp-sync"`；docs/specs/rust-u2-sync.md §2）。
//!
//! 七个成员是用户级/工作区级配置文件操作；`listWorkspaceMcpServerStatuses` 是运行态查询，
//! 转发给 workspace 实例的 `mcp/list`（TS 由 escodeAgentService 在独立进程执行）。

use std::path::Path;
use std::sync::Arc;

use escode_host_services::mcp_sync::McpSyncService;
use escode_host_services::skills::Compare;
use serde_json::Value;

use crate::channel::Channel;
use crate::domains_task_support::first_object;
use crate::forward::{self, RuntimePolicy};
use crate::session::{RpcError, handler_rpc};
use crate::wire::Wire;

fn failed(message: String) -> RpcError {
    RpcError::new("Error", message)
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(crate) fn mcp_sync_channel(
    home: &Path,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    let compare: Compare = Arc::new(crate::collation::compare_names);
    let service = Arc::new(McpSyncService::new(home.to_path_buf(), compare));
    let load = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .load_mcp_from_user_directory(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let statuses = {
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let (instances, options, params) =
                (instances.clone(), Arc::clone(&options), first_object(arg));
            Box::pin(async move {
                let target = forward::runtime_target(params)
                    .map_err(|(name, message)| RpcError::new(name, message))?;
                let instance = forward::runtime_instance(
                    &instances,
                    &options,
                    &target,
                    RuntimePolicy::StartIfNeeded,
                )
                .await?;
                let result = forward::request(&instance, "mcp/list", target.request, None).await?;
                Ok(Wire::Object(result))
            })
        })
    };
    let save = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .save_mcp_to_user_directory(&params)
                    .await
                    .map(|()| Wire::Undefined)
                    .map_err(failed)
            })
        })
    };
    let candidates = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .list_local_user_mcp_candidates()
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let remote_statuses = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .list_remote_user_mcp_statuses(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let export = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .export_mcp_servers(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let check_write_access = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .check_remote_user_mcp_write_access()
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let import = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let overwrite = js_truthy(params.get("overwrite").unwrap_or(&Value::Null));
                service
                    .import_mcp_servers(&params, overwrite)
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    Channel::new(
        "mcp-sync",
        vec![
            ("loadMcpFromUserDirectory", load),
            ("listWorkspaceMcpServerStatuses", statuses),
            ("saveMcpToUserDirectory", save),
            ("listLocalUserMcpCandidates", candidates),
            ("listRemoteUserMcpStatuses", remote_statuses),
            ("exportMcpServers", export),
            ("checkRemoteUserMcpWriteAccess", check_write_access),
            ("importMcpServers", import),
        ],
    )
}
