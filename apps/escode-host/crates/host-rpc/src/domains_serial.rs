//! `serial` channel（TS `ServiceChannels.Serial` = `"serial"`；docs/specs/rust-u2-serial.md）。
//!
//! **每连接一个实例**（`Channels::with_scoped`）：串口会话与 `seq` 都属于连接作用域
//! （用户决定 2026-10-17「每连接≈每窗口」）。
//!
//! 本片接 S1：`list` / `setWatching` / `onPorts`。S2–S4（会话、数据面、信号与循环）
//! 由 `respond` 回显式缺口并列出已实现集合。

use std::pin::Pin;
use std::sync::Arc;

use escode_host_services::serial::SerialService;
use escode_host_services::serial::enumerate::SysfsEnumerator;
use futures_util::stream;
use serde_json::Value;

use crate::channel::{Channel, ConnectionContext};
use crate::domains_task_support::first_object;
use crate::session::{EventStream, handler};

fn failed(error: String) -> (String, String) {
    ("Error".to_owned(), error)
}

pub(crate) fn serial_channel(_context: &ConnectionContext) -> Channel {
    let service = Arc::new(SerialService::new(Arc::new(SysfsEnumerator::default())));

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

    Channel::new(
        "serial",
        vec![("list", list), ("setWatching", set_watching)],
    )
    .with_events(vec![("onPorts", ports_event)])
    .with_close(shutdown)
}
