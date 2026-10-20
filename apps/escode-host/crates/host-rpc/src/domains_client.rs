//! `client-scenes` channel（docs/specs/rust-u2-client-config.md）。

use std::sync::Arc;

use escode_host_services::api_client::ApiError;
use escode_host_services::client_config::{ConfigError, parse_read_options};
use escode_host_services::client_services::ClientServices;

use crate::channel::Channel;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `ChannelServer` 只透传白名单键，`ApiError` 到 Renderer 时只剩 `name`/`message`/`status`（spec §6 ③）。
fn rpc_error(error: &ApiError) -> RpcError {
    let rpc = RpcError::new("ApiError", error.message.clone());
    match error.status {
        Some(status) => rpc.with("status", status.into()),
        None => rpc,
    }
}

fn config_error(error: &ConfigError) -> RpcError {
    let rpc = RpcError::new(error.name(), error.message());
    match error.status() {
        Some(status) => rpc.with("status", status.into()),
        None => rpc,
    }
}

/// 第一个实参：未传 / `undefined` → 缺省（TS `options = {}`）；其余（含 `null`）交给 schema 判定。
fn options_of(arg: Wire) -> Option<serde_json::Value> {
    match arg.into_args().into_iter().next() {
        None | Some(Wire::Undefined) => None,
        Some(Wire::Object(value)) => Some(value),
        Some(Wire::Str(text)) => Some(serde_json::Value::String(text)),
        Some(Wire::Int(number)) => Some(number.into()),
        Some(_) => Some(serde_json::Value::Null),
    }
}

fn missing() -> RpcError {
    RpcError::new(
        "NotImplemented",
        "client services are unavailable: Host started without runtime configuration.",
    )
}

fn list(services: Option<Arc<ClientServices>>) -> MethodHandler {
    handler_rpc(move |_arg: Wire| {
        let services = services.clone();
        Box::pin(async move {
            let services = services.ok_or_else(missing)?;
            services
                .scenes
                .list()
                .await
                .map(Wire::Object)
                .map_err(|error| rpc_error(&error))
        })
    })
}

fn get_snapshot(services: Option<Arc<ClientServices>>) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let services = services.clone();
        Box::pin(async move {
            let services = services.ok_or_else(missing)?;
            let force_refresh = parse_read_options(options_of(arg).as_ref())
                .map_err(|error| config_error(&error))?;
            services
                .config
                .get_snapshot(force_refresh)
                .await
                .map(|snapshot| Wire::Object(snapshot.to_json()))
                .map_err(|error| config_error(&error))
        })
    })
}

fn get_dynamic_workflow(services: Option<Arc<ClientServices>>) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let services = services.clone();
        Box::pin(async move {
            let services = services.ok_or_else(missing)?;
            let force_refresh = parse_read_options(options_of(arg).as_ref())
                .map_err(|error| config_error(&error))?;
            Ok(Wire::Object(
                services
                    .config
                    .get_dynamic_workflow(force_refresh)
                    .await
                    .to_json(),
            ))
        })
    })
}

pub(crate) fn client_config_channel(services: Option<Arc<ClientServices>>) -> Channel {
    Channel::new(
        "client-config",
        vec![
            ("getSnapshot", get_snapshot(services.clone())),
            (
                "getDynamicWorkflowClientConfig",
                get_dynamic_workflow(services),
            ),
        ],
    )
}

pub(crate) fn client_scenes_channel(services: Option<Arc<ClientServices>>) -> Channel {
    Channel::new("client-scenes", vec![("list", list(services))])
}
