//! `plugin-ui-sampling` channel（TS `ServiceChannels.PluginUiSampling` = `"plugin-ui-sampling"`）。
//!
//! `sample` / `cancelSampling` 转发 runtime 的采样账本与任务模型调用
//! （docs/specs/rust-mcp-ui.md §4 P4）；Host 不持有采样状态。

use std::sync::Arc;

use serde_json::json;

use crate::channel::MethodHandler;
use crate::domains_plugin_ui::{forward, required, scope};
use crate::domains_task_support::first_object;
use crate::forward::Extract;
use crate::session::{Channel, RpcError, handler_rpc};
use crate::wire::Wire;

pub(crate) fn plugin_ui_sampling_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    let handler = |method: &'static str, with_request: bool| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler_rpc(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                let params = first_object(arg);
                let mut body = scope(&params)?;
                body.insert(
                    "operationId".into(),
                    json!(required(&params, "operationId")?),
                );
                if with_request {
                    let request = params
                        .get("request")
                        .cloned()
                        .ok_or_else(|| RpcError::new("InvalidRequest", "request is required"))?;
                    body.insert("request".into(), request);
                }
                forward(&instances, &options, params, method, body, Extract::Result).await
            })
        })
    };
    let methods: Vec<(&'static str, MethodHandler)> = vec![
        ("sample", handler("mcp/uiSampling", true)),
        ("cancelSampling", handler("mcp/uiCancelSampling", false)),
    ];
    Channel::new("plugin-ui-sampling", methods)
}
