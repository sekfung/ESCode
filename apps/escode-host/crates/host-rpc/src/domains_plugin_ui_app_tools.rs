//! `plugin-ui-app-tools` channel（TS `ServiceChannels.PluginUiAppTools` = `"plugin-ui-app-tools"`）。
//!
//! 四个成员把登记、注销、认领与回传转发给 runtime 的会话登记表
//! （docs/specs/rust-mcp-ui.md §4 P3）；Host 不持有待执行调用。

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::channel::MethodHandler;
use crate::domains_plugin_ui::{forward, required, scope};
use crate::domains_task_support::first_object;
use crate::forward::Extract;
use crate::session::{Channel, RpcError, handler_rpc};
use crate::wire::Wire;

fn instance_body(params: &Value) -> Result<Map<String, Value>, RpcError> {
    let mut body = scope(params)?;
    body.insert("scopeId".into(), json!(required(params, "scopeId")?));
    body.insert(
        "generation".into(),
        params
            .get("generation")
            .cloned()
            .ok_or_else(|| RpcError::new("InvalidRequest", "generation is required"))?,
    );
    Ok(body)
}

pub(crate) fn plugin_ui_app_tools_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    let handler = |method: &'static str,
                   build: fn(&Value) -> Result<Map<String, Value>, RpcError>| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = first_object(arg);
                let body = build(&params)?;
                forward(&instances, &options, params, method, body, Extract::Result).await
            })
        })
    };
    let methods: Vec<(&'static str, MethodHandler)> = vec![
        (
            "registerAppTools",
            handler("mcp/uiRegisterAppTools", |params| {
                let mut body = instance_body(params)?;
                body.insert(
                    "tools".into(),
                    params
                        .get("tools")
                        .cloned()
                        .ok_or_else(|| RpcError::new("InvalidRequest", "tools is required"))?,
                );
                Ok(body)
            }),
        ),
        (
            "unregisterAppTools",
            handler("mcp/uiUnregisterAppTools", instance_body),
        ),
        (
            "claimAppToolCall",
            handler("mcp/uiClaimAppToolCall", |params| {
                let mut body = instance_body(params)?;
                body.insert("callId".into(), json!(required(params, "callId")?));
                Ok(body)
            }),
        ),
        (
            "resolveAppToolCall",
            handler("mcp/uiResolveAppToolCall", |params| {
                let mut body = instance_body(params)?;
                body.insert("callId".into(), json!(required(params, "callId")?));
                if let Some(result) = params.get("result") {
                    body.insert("result".into(), result.clone());
                }
                if let Some(error) = params.get("error") {
                    body.insert("error".into(), error.clone());
                }
                Ok(body)
            }),
        ),
    ];
    Channel::new("plugin-ui-app-tools", methods)
}
