//! `plugin-ui-bridge` channel（TS `ServiceChannels.PluginUiBridge` = `"plugin-ui-bridge"`）。
//!
//! 本阶段只登记 runtime 已实现的成员：实例校验/回收/关闭、资源读/列表/模板、页面工具调用与取消。
//! 订阅、App 工具与采样仍为 Pending，不注册以保持缺口显式（docs/specs/rust-mcp-ui.md §4）。

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::channel::MethodHandler;
use crate::domains_task_support::first_object;
use crate::forward::{Extract, Shaped, runtime_call_shaped};
use crate::plugin_ui_prepare::prepare_sandbox;
use crate::session::{Channel, RpcError, RpcResult, handler_rpc};
use crate::wire::Wire;

pub(crate) fn required(params: &Value, key: &'static str) -> Result<String, RpcError> {
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

pub(crate) fn optional(params: &Value, body: &mut Map<String, Value>, key: &str) {
    if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
        body.insert(key.to_owned(), value.clone());
    }
}

/// 订阅/退订：实例作用域 + 沙箱作用域 id + 代际 + 资源地址。
fn subscription_body(params: &Value) -> Result<Map<String, Value>, RpcError> {
    let mut body = scope(params)?;
    body.insert("scopeId".into(), json!(required(params, "scopeId")?));
    body.insert(
        "generation".into(),
        params
            .get("generation")
            .cloned()
            .ok_or_else(|| RpcError::new("InvalidRequest", "generation is required"))?,
    );
    body.insert("uri".into(), json!(required(params, "uri")?));
    Ok(body)
}

pub(crate) async fn forward(
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

    let list_surfaces = {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = first_object(arg);
                let result = forward(
                    &instances,
                    &options,
                    params,
                    "plugins/listUiSurfaces",
                    Map::new(),
                    Extract::Result,
                )
                .await?;
                let Wire::Object(value) = result else {
                    return Err(RpcError::new(
                        "Fatal",
                        "plugins/listUiSurfaces returned no result",
                    ));
                };
                let surfaces = value["surfaces"].as_array().cloned().unwrap_or_default();
                Ok(Wire::Array(
                    surfaces.into_iter().map(Wire::Object).collect(),
                ))
            })
        })
    };

    let subscription = |method: &'static str| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = first_object(arg);
                let body = subscription_body(&params)?;
                forward(&instances, &options, params, method, body, Extract::Result).await?;
                Ok(Wire::Undefined)
            })
        })
    };

    let methods: Vec<(&'static str, MethodHandler)> = vec![
        ("prepareSandbox", prepare),
        ("listSurfaces", list_surfaces),
        ("subscribeResource", subscription("mcp/uiSubscribeResource")),
        (
            "unsubscribeResource",
            subscription("mcp/uiUnsubscribeResource"),
        ),
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
