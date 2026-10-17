//! `serial` channel（TS `ServiceChannels.Serial` = `"serial"`；docs/specs/rust-u2-serial.md）。
//!
//! **每连接一个实例**（`Channels::with_scoped`）：串口会话与 `seq` 都属于连接作用域
//! （用户决定 2026-10-17「每连接≈每窗口」）。
//!
//! 已接 S1 + S2：`list` / `setWatching` / `onPorts` 与会话（`open` / `close` / `getSnapshot` /
//! `listSessions` / `onStatus`）。S3–S4（数据面、信号与循环）由 `respond` 回显式缺口。

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

    Channel::new(
        "serial",
        vec![
            ("list", list),
            ("setWatching", set_watching),
            ("open", open),
            ("close", close),
            ("getSnapshot", get_snapshot),
            ("listSessions", list_sessions),
        ],
    )
    .with_events(vec![("onPorts", ports_event), ("onStatus", status_event)])
    .with_close(shutdown)
}
