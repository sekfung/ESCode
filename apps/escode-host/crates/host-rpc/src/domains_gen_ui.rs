//! `gen-ui` channel（TS `ServiceChannels.GenUi` = `"gen-ui"`；docs/specs/rust-u2-gen-ui.md）。
//!
//! 五个成员全是 Host 本地能力（不经运行时）：`readDocument` 是同步受限读（走 `spawn_blocking_io`），
//! 四个状态成员是异步文件操作，`onStateChanged` 是进程级广播事件。
//! `prepareSandbox` 不在本域——它要在 Electron Main 注册 webview 沙箱（边界子 spec §3.2）。

use std::sync::Arc;

use escode_host_services::gen_ui::GenUiService;
use futures_util::stream;
use serde_json::Value;

use crate::domains_file::spawn_blocking_io;
use crate::domains_task_support::first_object;
use crate::session::{Channel, EventStream, handler};
use crate::wire::Wire;

fn failed(error: String) -> (String, String) {
    ("Error".to_owned(), error)
}

/// 缺 `dataBaseDir` 时方法表照常登记（契约对账要求），调用一律回结构化缺口。
fn gap_channel() -> Channel {
    let gap = "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.";
    let missing = || {
        handler(move |_arg: Wire| {
            let gap = gap.to_owned();
            Box::pin(async move { Err(("InvalidRequest".to_owned(), gap)) })
        })
    };
    Channel::new(
        "gen-ui",
        vec![
            ("readDocument", missing()),
            ("getState", missing()),
            ("setState", missing()),
            ("listState", missing()),
        ],
    )
    .with_events(vec![(
        "onStateChanged",
        Arc::new(|_arg: Wire| Ok(Box::pin(stream::pending()) as EventStream)),
    )])
}

pub(crate) fn gen_ui_channel(data_base_dir: Option<std::path::PathBuf>) -> Channel {
    let Some(data_base_dir) = data_base_dir else {
        return gap_channel();
    };
    let service = Arc::new(GenUiService::new(&data_base_dir));

    let read_document = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let document =
                    spawn_blocking_io(move || service.read_document(&params).map_err(failed))
                        .await?;
                Ok::<Wire, (String, String)>(Wire::Object(document))
            })
        })
    };
    let get_state = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let state = service.get_state(&params).await.map_err(failed)?;
                Ok(Wire::Object(state))
            })
        })
    };
    let set_state = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.set_state(&params).await.map_err(failed)?;
                Ok(Wire::Undefined)
            })
        })
    };
    let list_state = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let entries = service.list_state(&params).await.map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Array(
                    entries.into_iter().map(Wire::Object).collect(),
                ))
            })
        })
    };
    // 进程级广播：每个订阅者一条独立流；无静态事件参数。
    let change = {
        let service = Arc::clone(&service);
        Arc::new(move |_arg: Wire| {
            let receiver = service.subscribe();
            let stream: EventStream =
                Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver
                        .recv()
                        .await
                        .map(|value: Value| (Wire::Object(value), receiver))
                }));
            Ok(stream)
        })
    };

    Channel::new(
        "gen-ui",
        vec![
            ("readDocument", read_document),
            ("getState", get_state),
            ("setState", set_state),
            ("listState", list_state),
        ],
    )
    .with_events(vec![("onStateChanged", change)])
}
