//! `serial` channel（TS `ServiceChannels.Serial` = `"serial"`；docs/specs/rust-u2-serial.md）。
//!
//! **每连接一个实例**（`Channels::with_scoped`）：串口会话与 `seq` 都属于连接作用域
//! （用户决定 2026-10-17「每连接≈每窗口」）。
//!
//! S1–S4 全部接通：枚举/轮询、会话（含自动重连）、数据面、信号（DTR/RTS + 复位脉冲）与定时循环。

use std::pin::Pin;
use std::sync::Arc;

use escode_host_services::serial::SerialService;
use escode_host_services::serial::enumerate::SysfsEnumerator;
use escode_host_services::serial::signals::Pulse;
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

/// `bytes` 参数：对象里的 `Uint8Array` 由 `@escode/rpc` 加协议标记（`wire_bytes`）。
fn param_bytes(params: &Value) -> Result<Vec<u8>, RpcError> {
    crate::wire_bytes::decode(&params["bytes"]).map_err(|message| {
        RpcError::new("SerialError", message).with("code", Value::from("invalidInput"))
    })
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
                Box::pin(async move { service.shutdown().await })
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

    let set_signals = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let dtr = params.get("dtr").and_then(Value::as_bool);
                let rts = params.get("rts").and_then(Value::as_bool);
                let pulse = params
                    .get("pulse")
                    .and_then(Value::as_str)
                    .and_then(Pulse::from_name);
                let signals = service
                    .set_signals(&path, dtr, rts, pulse)
                    .await
                    .map_err(serial_error)?;
                Ok(crate::wire::Wire::Object(signals.to_json()))
            })
        })
    };
    let start_loop = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let bytes = param_bytes(&params)?;
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let interval_ms = params["intervalMs"].as_u64().unwrap_or(0);
                // TS：`count !== undefined` 时必须是整数 ≥ 1；JSON null 不是 undefined，按非法处理。
                let count = params.get("count").map(|value| value.as_u64().unwrap_or(0));
                service
                    .start_loop(&path, bytes, interval_ms, count)
                    .map_err(serial_error)?;
                Ok(crate::wire::Wire::Undefined)
            })
        })
    };
    let stop_loop = {
        let service = Arc::clone(&service);
        handler(move |arg: crate::wire::Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.stop_loop(params["path"].as_str().unwrap_or_default());
                Ok(crate::wire::Wire::Undefined)
            })
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
            ("setSignals", set_signals),
            ("startLoop", start_loop),
            ("stopLoop", stop_loop),
        ],
    )
    .with_events(vec![
        ("onPorts", ports_event),
        ("onStatus", status_event),
        ("onData", data_event),
    ])
    .with_close(shutdown)
}
