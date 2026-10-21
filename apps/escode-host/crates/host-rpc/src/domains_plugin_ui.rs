//! `plugin-ui-bridge` channel（TS `ServiceChannels.PluginUiBridge` = `"plugin-ui-bridge"`）。
//!
//! 本阶段只登记 runtime 已实现的成员：实例校验/回收/关闭、资源读/列表/模板、页面工具调用与取消。
//! `prepareSandbox`（Host 组装壳 + Main 注册）、订阅、App 工具与采样仍为 Pending，
//! 不注册以保持缺口显式（docs/specs/rust-mcp-ui.md §4）。

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::channel::MethodHandler;
use crate::domains_task_support::first_object;
use crate::forward::{Extract, Shaped, runtime_call_shaped};
use crate::plugin_ui_meta;
use crate::session::{Channel, RpcError, RpcResult, handler_rpc};
use crate::wire::Wire;

fn required(params: &Value, key: &'static str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| RpcError::new("InvalidRequest", format!("{key} is required")))
}

/// 所有成员共用 `escodeMcpUiPluginScopeShape`：实例凭证 + 会话 + 插件 + namespaced server。
fn scope(params: &Value) -> Result<Map<String, Value>, RpcError> {
    let instance = params
        .get("instance")
        .cloned()
        .ok_or_else(|| RpcError::new("InvalidRequest", "instance is required"))?;
    let mut body = Map::new();
    body.insert("instance".into(), instance);
    for key in ["sessionId", "pluginId", "serverName"] {
        body.insert(key.into(), json!(required(params, key)?));
    }
    Ok(body)
}

fn optional(params: &Value, body: &mut Map<String, Value>, key: &str) {
    if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
        body.insert(key.to_owned(), value.clone());
    }
}

async fn forward(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    params: Value,
    method: &'static str,
    body: Map<String, Value>,
    extract: Extract,
) -> RpcResult {
    runtime_call_shaped(
        instances,
        options,
        params,
        Shaped {
            method,
            body: Value::Object(body),
            with_workspace: true,
            trace: None,
        },
        extract,
    )
    .await
}

pub(crate) fn plugin_ui_bridge_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
    main_broker: Option<Arc<escode_host_control::main_broker::MainBroker>>,
) -> Channel {
    let prepare = {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            let main_broker = main_broker.clone();
            Box::pin(async move {
                prepare_sandbox(main_broker, &instances, &options, first_object(arg)).await
            })
        })
    };
    macro_rules! scoped {
        ($method:literal, $build:expr, $extract:expr) => {{
            let instances = instances.clone();
            let options = Arc::clone(&options);
            handler_rpc(move |arg: Wire| {
                let instances = instances.clone();
                let options = Arc::clone(&options);
                Box::pin(async move {
                    let params = first_object(arg);
                    let build: fn(&Value) -> Result<Map<String, Value>, RpcError> = $build;
                    let body = build(&params)?;
                    forward(&instances, &options, params, $method, body, $extract).await
                })
            })
        }};
    }

    let validate = {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = first_object(arg);
                let body = scope(&params)?;
                forward(
                    &instances,
                    &options,
                    params,
                    "mcp/uiValidateInstance",
                    body,
                    Extract::Result,
                )
                .await?;
                Ok(Wire::Undefined)
            })
        })
    };
    let close = {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = first_object(arg);
                let body = scope(&params)?;
                forward(
                    &instances,
                    &options,
                    params,
                    "mcp/uiCloseInstance",
                    body,
                    Extract::Result,
                )
                .await?;
                Ok(Wire::Undefined)
            })
        })
    };

    let methods: Vec<(&'static str, MethodHandler)> = vec![
        ("prepareSandbox", prepare),
        ("validateInstance", validate),
        ("closeInstance", close),
        (
            "recycleInstance",
            scoped!(
                "mcp/uiCloseInstance",
                |params: &Value| {
                    let mut body = scope(params)?;
                    body.insert("onlyIfIdle".into(), Value::Bool(true));
                    Ok(body)
                },
                Extract::Field("closed")
            ),
        ),
        (
            "readResource",
            scoped!(
                "mcp/readResource",
                |params: &Value| {
                    let mut body = scope(params)?;
                    body.insert("uri".into(), json!(required(params, "uri")?));
                    Ok(body)
                },
                Extract::Result
            ),
        ),
        (
            "callTool",
            scoped!(
                "mcp/uiCallTool",
                |params: &Value| {
                    let mut body = scope(params)?;
                    body.insert("toolName".into(), json!(required(params, "toolName")?));
                    body.insert("callId".into(), json!(required(params, "callId")?));
                    optional(params, &mut body, "arguments");
                    Ok(body)
                },
                Extract::Result
            ),
        ),
        (
            "cancelToolCall",
            scoped!(
                "mcp/uiCancelCall",
                |params: &Value| {
                    let mut body = scope(params)?;
                    body.insert("callId".into(), json!(required(params, "callId")?));
                    Ok(body)
                },
                Extract::Result
            ),
        ),
        (
            "listResources",
            scoped!(
                "mcp/uiListResources",
                |params: &Value| {
                    let mut body = scope(params)?;
                    optional(params, &mut body, "cursor");
                    Ok(body)
                },
                Extract::Result
            ),
        ),
        (
            "listResourceTemplates",
            scoped!(
                "mcp/uiListResourceTemplates",
                |params: &Value| {
                    let mut body = scope(params)?;
                    optional(params, &mut body, "cursor");
                    Ok(body)
                },
                Extract::Result
            ),
        ),
    ];
    Channel::new("plugin-ui-bridge", methods)
}

/// `prepareSandbox`：Host 取实例与 HTML、归一化资源 `_meta`，真实 webview 注册交给 Main。
async fn prepare_sandbox(
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
