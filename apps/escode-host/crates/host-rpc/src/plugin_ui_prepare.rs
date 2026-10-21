//! `plugin-ui-bridge.prepareSandbox`：Host 取实例与 HTML、归一化资源 `_meta`，
//! 真实 webview 注册交给 Main（docs/specs/rust-mcp-ui.md §3.1）。

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::domains_plugin_ui::{forward, optional, required};
use crate::forward::Extract;
use crate::plugin_ui_meta;
use crate::session::{RpcError, RpcResult};
use crate::wire::Wire;

/// `prepareSandbox`：Host 取实例与 HTML、归一化资源 `_meta`，真实 webview 注册交给 Main。
pub(crate) async fn prepare_sandbox(
    broker: Option<Arc<escode_host_control::main_broker::MainBroker>>,
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    params: Value,
) -> RpcResult {
    let Some(broker) = broker else {
        return Err(RpcError::new(
            "NotImplemented",
            "plugin-ui-bridge.prepareSandbox requires Electron Main; Main did not declare mainServices.",
        ));
    };
    let workspace_path = required(&params, "workspacePath")?;
    let session_id = required(&params, "sessionId")?;
    let plugin_id = required(&params, "pluginId")?;
    let server_name = required(&params, "serverName")?;
    let scope_id = required(&params, "scopeId")?;
    let resource_uri = required(&params, "resourceUri")?;
    let owner = params["ownerWebContentsId"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            RpcError::new(
                "InvalidRequest",
                "ownerWebContentsId must be a positive integer",
            )
        })?;

    let mut open = Map::new();
    open.insert("sessionId".into(), json!(session_id));
    open.insert("pluginId".into(), json!(plugin_id));
    open.insert("serverName".into(), json!(server_name));
    open.insert("scopeId".into(), json!(scope_id));
    open.insert("resourceUri".into(), json!(resource_uri));
    open.insert("ownerWebContentsId".into(), json!(owner));
    optional(&params, &mut open, "accountContext");
    let instance = forward(
        instances,
        options,
        params.clone(),
        "mcp/uiOpenInstance",
        open,
        Extract::Result,
    )
    .await?;
    let Wire::Object(instance) = instance else {
        return Err(RpcError::new(
            "Fatal",
            "mcp/uiOpenInstance returned no instance",
        ));
    };

    let mut read = Map::new();
    read.insert("instance".into(), instance.clone());
    read.insert("sessionId".into(), json!(session_id));
    read.insert("pluginId".into(), json!(plugin_id));
    read.insert("serverName".into(), json!(server_name));
    read.insert("uri".into(), json!(resource_uri));
    let raw = match forward(
        instances,
        options,
        params.clone(),
        "mcp/readResource",
        read,
        Extract::Result,
    )
    .await?
    {
        Wire::Object(value) => value,
        _ => {
            return Err(RpcError::new(
                "Fatal",
                "mcp/readResource returned no result",
            ));
        }
    };
    let picked = plugin_ui_meta::pick_html(&raw, &resource_uri)
        .map_err(|message| RpcError::new("InvalidRequest", message))?;
    // 资源项没带 `_meta` 时翻 resources/list 找同 uri 条目的声明（最多 8 页），失败视为没有声明。
    let meta = match picked.meta {
        Some(meta) => Some(meta),
        None => {
            list_level_meta(
                instances,
                options,
                &params,
                &instance,
                &session_id,
                &plugin_id,
                &server_name,
                &resource_uri,
            )
            .await
        }
    };

    let mut register = json!({
        "instance": instance,
        "workspacePath": workspace_path,
        "ownerWebContentsId": owner,
        "sessionId": session_id,
        "pluginId": plugin_id,
        "serverName": server_name,
        "scopeId": scope_id,
        "html": picked.html,
    });
    if let Some(identity) = params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        register["workspaceIdentity"] = json!(identity);
    }
    if let Some(meta) = meta {
        for key in [
            "csp",
            "cspRelaxations",
            "permissions",
            "prefersBorder",
            "heightHint",
            "minFrameHeight",
            "showInline",
        ] {
            if let Some(value) = meta.get(key) {
                register[key] = value.clone();
            }
        }
    }
    // 工具级声明只在资源两处都没有对应字段时兜底。
    if register.get("csp").is_none()
        && let Some(csp) = params.get("csp")
    {
        register["csp"] = csp.clone();
    }
    if register.get("prefersBorder").is_none()
        && let Some(prefers) = params.get("prefersBorder")
    {
        register["prefersBorder"] = prefers.clone();
    }
    broker
        .request("plugin-ui-bridge.prepareSandbox", register)
        .await
        .map(Wire::Object)
        .map_err(|error| RpcError::new("Fatal", error))
}

#[allow(clippy::too_many_arguments)]
async fn list_level_meta(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    routing: &Value,
    instance: &Value,
    session_id: &str,
    plugin_id: &str,
    server_name: &str,
    resource_uri: &str,
) -> Option<Value> {
    let mut cursor: Option<String> = None;
    for _ in 0..8 {
        let mut body = Map::new();
        body.insert("instance".into(), instance.clone());
        body.insert("sessionId".into(), json!(session_id));
        body.insert("pluginId".into(), json!(plugin_id));
        body.insert("serverName".into(), json!(server_name));
        if let Some(cursor) = &cursor {
            body.insert("cursor".into(), json!(cursor));
        }
        let result = forward(
            instances,
            options,
            routing.clone(),
            "mcp/uiListResources",
            body,
            Extract::Result,
        )
        .await
        .ok()?;
        let Wire::Object(result) = result else {
            return None;
        };
        if let Some(entry) = result["resources"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["uri"].as_str() == Some(resource_uri))
        {
            return plugin_ui_meta::normalize_resource_meta(&entry["_meta"]);
        }
        cursor = result["nextCursor"].as_str().map(str::to_owned);
        cursor.as_ref()?;
    }
    None
}
