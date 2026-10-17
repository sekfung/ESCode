//! `serial` channel（TS `ServiceChannels.Serial` = `"serial"`；docs/specs/rust-u2-serial.md）。
//!
//! **每连接一个实例**（`Channels::with_scoped`）：串口会话与 `seq` 都属于连接作用域
//! （用户决定 2026-10-17「每连接≈每窗口」）。
//!
//! 已接 S1 + S2 + S3：枚举/轮询、会话（含自动重连）与数据面（`write` / `clear` / `onData`）。
//! S4（信号与循环）由 `respond` 回显式缺口。

use std::pin::Pin;
use std::sync::Arc;

use escode_host_services::serial::SerialService;
use escode_host_services::serial::enumerate::SysfsEnumerator;
use escode_host_services::serial::transport::NativeTransport;
use futures_util::stream;
use serde_json::Value;

use crate::channel::{Channel, ConnectionContext};
use crate::domains_task_support::first_object;
use crate::session::{EventStream, RpcError, handler, handler_rpc};

fn failed(error: String) -> (String, String) {
    ("Error".to_owned(), error)
}

/// TS `SerialError`：`name = "SerialError"` 且带稳定 `code`（busy/denied/notFound/…）。
fn serial_error(error: escode_host_services::serial::transport::SerialOpenError) -> RpcError {
    RpcError::new("SerialError", error.message).with("code", Value::from(error.code))
}

/// `bytes` 参数：RPC 对**对象里的** `Uint8Array` 会加标记 `{__escode_rpc_nested_uint8array_v1, base64}`；
/// 纯数字数组（测试或 JSON 直发）也接受。
fn param_bytes(params: &Value) -> Result<Vec<u8>, RpcError> {
    use base64::Engine as _;
    let raw = &params["bytes"];
    if raw
        .get("__escode_rpc_nested_uint8array_v1")
        .and_then(Value::as_bool)
        == Some(true)
        && let Some(text) = raw.get("base64").and_then(Value::as_str)
    {
        return base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|error| {
                RpcError::new("SerialError", format!("Invalid bytes: {error}"))
                    .with("code", Value::from("invalidInput"))
            });
    }
    if let Some(items) = raw.as_array() {
        return Ok(items
            .iter()
            .filter_map(Value::as_u64)
            .map(|byte| byte as u8)
            .collect());
    }
    if raw.is_null() {
        return Ok(Vec::new());
    }
    Err(
        RpcError::new("SerialError", "bytes must be a Uint8Array".to_owned())
            .with("code", Value::from("invalidInput")),
    )
}

pub(crate) fn serial_channel(_context: &ConnectionContext) -> Channel {
    let service = Arc::new(SerialService::new(
        Arc::new(SysfsEnumerator::default()),
        Arc::new(NativeTransport),
    ));

    let list = {
        let service = Arc::clone(&service);
        handler(move |_arg: crate::wire::Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                let ports = service.list().await.map_err(failed)?;
                Ok::<crate::wire::Wire, (String, String)>(crate::wire::Wire::Array(
                    ports
                        .into_iter()
                        .map(|port| crate::wire::Wire::Object(Value::Object(port)))
                        .collect(),
                ))
            })
        })
    };
    let set_watching = {
        let service = Arc::clone(&service);
        handler(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .set_watching(params["watching"].as_bool().unwrap_or(false))
                    .await;
                Ok(crate::wire::Wire::Undefined)
            })
        })
    };
    let ports_event = {
        let service = Arc::clone(&service);
        Arc::new(move |_arg: crate::wire::Wire| {
            let receiver = service.subscribe_ports();
            let stream: EventStream =
                Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver
                        .recv()
                        .await
                        .map(|value: Value| (crate::wire::Wire::Object(value), receiver))
                }));
            Ok(stream)
        })
    };
    let shutdown = {
        let service = Arc::clone(&service);
        Arc::new(
            move || -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                let service = Arc::clone(&service);
                Box::pin(async move { service.shutdown() })
            },
        )
    };

    let open = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                service
                    .open(&path, params.get("config").unwrap_or(&Value::Null))
                    .await
                    .map_err(serial_error)?;
                Ok(crate::wire::Wire::Undefined)
            })
        })
    };
    let close = {
        let service = Arc::clone(&service);
        handler(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .close(params["path"].as_str().unwrap_or_default())
                    .await;
                Ok(crate::wire::Wire::Undefined)
            })
        })
    };
    let get_snapshot = {
        let service = Arc::clone(&service);
        handler(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let snapshot = service.get_snapshot(params["path"].as_str().unwrap_or_default());
                Ok::<crate::wire::Wire, (String, String)>(crate::wire::Wire::Object(snapshot))
            })
        })
    };
    let list_sessions = {
        let service = Arc::clone(&service);
        handler(move |_arg: crate::wire::Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                Ok::<crate::wire::Wire, (String, String)>(crate::wire::Wire::Array(
                    service
                        .list_sessions()
                        .into_iter()
                        .map(crate::wire::Wire::Object)
                        .collect(),
                ))
            })
        })
    };
    let status_event = {
        let service = Arc::clone(&service);
        Arc::new(move |_arg: crate::wire::Wire| {
            let receiver = service.subscribe_status();
            let stream: EventStream =
                Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver
                        .recv()
                        .await
                        .map(|value: Value| (crate::wire::Wire::Object(value), receiver))
                }));
            Ok(stream)
        })
    };

    let write = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let bytes = param_bytes(&params)?;
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let source = params["source"].as_str().unwrap_or("user").to_owned();
                let session_id = params["sessionId"].as_str().map(str::to_owned);
                let seq = service
                    .write(&path, bytes, &source, session_id)
                    .await
                    .map_err(serial_error)?;
                Ok(crate::wire::Wire::Object(serde_json::json!({"seq": seq})))
            })
        })
    };
    let clear = {
        let service = Arc::clone(&service);
        handler(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.clear(params["path"].as_str().unwrap_or_default());
                Ok(crate::wire::Wire::Undefined)
            })
        })
    };
    let data_event = {
        let service = Arc::clone(&service);
        Arc::new(move |_arg: crate::wire::Wire| {
            let receiver = service.subscribe_data();
            let stream: EventStream =
                Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver
                        .recv()
                        .await
                        .map(|value: Value| (crate::wire::Wire::Object(value), receiver))
                }));
            Ok(stream)
        })
    };

    Channel::new(
        "serial",
        vec![
            ("list", list),
            ("setWatching", set_watching),
            ("open", open),
            ("close", close),
            ("getSnapshot", get_snapshot),
            ("listSessions", list_sessions),
            ("write", write),
            ("clear", clear),
        ],
    )
    .with_events(vec![
        ("onPorts", ports_event),
        ("onStatus", status_event),
        ("onData", data_event),
    ])
    .with_close(shutdown)
}
