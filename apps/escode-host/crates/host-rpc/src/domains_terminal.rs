//! `terminal` channel（TS `ServiceChannels.Terminal` = `"terminal"`；docs/specs/rust-u2-terminal.md）。
//!
//! 业务在 `escode_host_services::terminal::TerminalService`（Host 内唯一实例）；这里只做参数解包、
//! settings 读取与事件流适配。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::terminal::{CreateParams, TerminalService};
use futures_util::stream;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::domains_task_support::first_object;
use crate::session::{Channel, EventHandler, EventStream, handler};
use crate::wire::Wire;

/// 组合根构造 `terminal` channel。`home` 与 settings 域同源（用于读 `terminalFontFamily` 等）。
pub fn terminal_channel(home: PathBuf) -> Channel {
    terminal_channel_with(home, Arc::new(TerminalService::new()))
}

pub fn terminal_channel_with(home: PathBuf, service: Arc<TerminalService>) -> Channel {
    let create = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, home, params) = (Arc::clone(&service), home.clone(), first_object(arg));
            Box::pin(async move {
                let create = CreateParams {
                    cols: dimension(&params, "cols"),
                    rows: dimension(&params, "rows"),
                    cwd: params.get("cwd").and_then(Value::as_str).map(str::to_owned),
                };
                // TS：settings 读失败按「继承系统 profile、无自定义字体」。
                let settings = escode_host_services::settings::read(&home)
                    .await
                    .unwrap_or_else(|_| json!({}));
                let env = escode_cli_core_api::env::vars();
                service
                    .create(create, &env, &settings)
                    .await
                    .map(Wire::Object)
                    .map_err(error)
            })
        })
    };
    let write = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let data = params["data"].as_str().unwrap_or_default().to_owned();
                service
                    .write(&text(&params, "id"), data)
                    .await
                    .map(|()| Wire::Undefined)
                    .map_err(error)
            })
        })
    };
    let resize = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .resize(
                        &text(&params, "id"),
                        dimension(&params, "cols"),
                        dimension(&params, "rows"),
                    )
                    .await
                    .map(|()| Wire::Undefined)
                    .map_err(error)
            })
        })
    };
    let dispose = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.dispose(&text(&params, "id")).await;
                Ok(Wire::Undefined)
            })
        })
    };
    Channel::new(
        "terminal",
        vec![
            ("create", create),
            ("write", write),
            ("resize", resize),
            ("dispose", dispose),
        ],
    )
    .with_events(vec![
        ("onDynamicData", data_event(&service)),
        ("onDynamicExit", exit_event(&service)),
    ])
}

fn text(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 终端尺寸：JS number → u16（越界按饱和处理，不让 pty 层收到非法值）。
fn dimension(params: &Value, key: &str) -> u16 {
    let value = params.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    value.clamp(0.0, f64::from(u16::MAX)) as u16
}

fn error(error: anyhow::Error) -> (String, String) {
    ("Error".to_owned(), format!("{error:#}"))
}

/// `onDynamicX(id)` 的实参是终端 id 字符串。
fn id_of(arg: Wire) -> String {
    match arg.into_args().into_iter().next() {
        Some(Wire::Str(id)) => id,
        Some(other) => other
            .into_object()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default(),
        None => String::new(),
    }
}

fn data_event(service: &Arc<TerminalService>) -> EventHandler {
    let service = Arc::clone(service);
    Arc::new(move |arg: Wire| {
        let receiver = service
            .listen_data(&id_of(arg))
            .map_err(|e| e.to_string())?;
        Ok(to_stream(receiver, Wire::Str))
    })
}

fn exit_event(service: &Arc<TerminalService>) -> EventHandler {
    let service = Arc::clone(service);
    Arc::new(move |arg: Wire| {
        let receiver = service
            .listen_exit(&id_of(arg))
            .map_err(|e| e.to_string())?;
        Ok(to_stream(receiver, Wire::Int))
    })
}

fn to_stream<T: Send + 'static>(receiver: mpsc::Receiver<T>, wrap: fn(T) -> Wire) -> EventStream {
    Box::pin(stream::unfold(receiver, move |mut receiver| async move {
        receiver.recv().await.map(|value| (wrap(value), receiver))
    }))
}
