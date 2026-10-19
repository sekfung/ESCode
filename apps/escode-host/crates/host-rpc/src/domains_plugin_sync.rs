//! `plugin-sync` channel（TS `ServiceChannels.PluginSync`；docs/specs/rust-u2-plugin-sync.md）。

use std::path::Path;
use std::sync::Arc;

use escode_host_services::plugin_sync::PluginSyncService;
use escode_host_services::skills::Compare;
use serde_json::Value;

use crate::domains_task_support::first_object;
use crate::session::{Channel, RpcError, handler, handler_rpc};
use crate::wire::Wire;

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(crate) fn plugin_sync_channel(home: &Path) -> Channel {
    let compare: Compare = Arc::new(crate::collation::compare_names);
    let service = Arc::new(PluginSyncService::new(home.to_path_buf(), compare));
    let list_candidates = {
        let service = Arc::clone(&service);
        handler(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .list_local_user_plugin_candidates()
                    .await
                    .map(Wire::Object)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        })
    };
    let list_statuses = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .list_remote_user_plugin_statuses(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        })
    };
    let export = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let result = service
                    .export_plugins_archive(&params)
                    .await
                    .map_err(|message| ("Error".to_owned(), message))?;
                Ok(Wire::Object(serde_json::json!({
                    "archive": crate::wire_bytes::encode(&result.archive),
                    "archiveBytes": result.archive_bytes,
                    "plugins": result.plugins,
                })))
            })
        })
    };
    let import = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let archive = crate::wire_bytes::decode(&params["archive"])
                    .map_err(|message| RpcError::new("Error", message))?;
                let overwrite = js_truthy(params.get("overwrite").unwrap_or(&Value::Null));
                service
                    .import_plugins_archive(archive, overwrite)
                    .await
                    .map(Wire::Object)
                    .map_err(|message| RpcError::new("Error", message))
            })
        })
    };
    let check_access = {
        let service = Arc::clone(&service);
        handler(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .check_remote_user_plugin_write_access()
                    .await
                    .map(Wire::Object)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        })
    };
    let export_marketplace = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let result = service
                    .export_marketplace_source_archive(&params)
                    .await
                    .map_err(|message| ("Error".to_owned(), message))?;
                Ok(Wire::Object(serde_json::json!({
                    "archive": crate::wire_bytes::encode(&result.archive),
                    "archiveBytes": result.archive_bytes,
                    "marketplaceId": result.marketplace_id,
                    "pluginNames": result.plugin_names,
                })))
            })
        })
    };
    let import_marketplace = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let archive = crate::wire_bytes::decode(&params["archive"])
                    .map_err(|message| RpcError::new("Error", message))?;
                let overwrite = js_truthy(params.get("overwrite").unwrap_or(&Value::Null));
                service
                    .import_marketplace_source_archive(archive, overwrite)
                    .await
                    .map(Wire::Object)
                    .map_err(|message| RpcError::new("Error", message))
            })
        })
    };
    Channel::new(
        "plugin-sync",
        vec![
            ("listLocalUserPluginCandidates", list_candidates),
            ("listRemoteUserPluginStatuses", list_statuses),
            ("exportPluginsArchive", export),
            ("importPluginsArchive", import),
            ("checkRemoteUserPluginWriteAccess", check_access),
            ("exportMarketplaceSourceArchive", export_marketplace),
            ("importMarketplaceSourceArchive", import_marketplace),
        ],
    )
}
