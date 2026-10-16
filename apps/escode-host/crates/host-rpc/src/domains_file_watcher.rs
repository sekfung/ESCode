//! `file-watcher` channel（TS `ServiceChannels.FileWatcher` = `"file-watcher"`；docs/specs/rust-u2-file-watcher.md）。

use escode_host_control::logging;
use std::sync::Arc;

use escode_host_services::file_watcher::FileWatcherService;
use futures_util::stream;
use serde_json::{Value, json};

use crate::domains_task_support::first_object;
use crate::session::{Channel, EventStream, handler};
use crate::wire::Wire;

/// 组合根构造 `file-watcher` channel（Host 内唯一的服务实例）。
pub fn file_watcher_channel() -> Channel {
    file_watcher_channel_with(Arc::new(FileWatcherService::new()))
}

pub fn file_watcher_channel_with(service: Arc<FileWatcherService>) -> Channel {
    let watch = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let path = params["path"].as_str().unwrap_or_default();
                let recursive = params["recursive"].as_bool().unwrap_or(false);
                service
                    .watch(path, recursive)
                    .await
                    .map(|id| Wire::Object(json!({"id": id})))
                    .map_err(|error| ("Error".to_owned(), format!("{error:#}")))
            })
        })
    };
    let unwatch = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.unwatch(params["id"].as_str().unwrap_or_default());
                Ok(Wire::Undefined)
            })
        })
    };
    let dispose_all = {
        let service = Arc::clone(&service);
        handler(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service.dispose_all();
                Ok(Wire::Undefined)
            })
        })
    };
    let change = {
        let service = Arc::clone(&service);
        Arc::new(move |arg: Wire| {
            let id = match arg.into_args().into_iter().next() {
                Some(Wire::Str(id)) => id,
                _ => String::new(),
            };
            let stream: EventStream = match service.listen(&id) {
                Some(receiver) => Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver
                        .recv()
                        .await
                        .map(|value: Value| (Wire::Object(value), receiver))
                })),
                None => {
                    // watch() 返回后、订阅前底层可能已因目录删除等被清理：stale id 可恢复，
                    // 不能把错误抛进事件订阅链路（TS：warn + Event.None）。
                    logging::warn(
                        "file-watcher",
                        format!("ignoring stale file watcher subscription id={id}"),
                    );
                    Box::pin(stream::pending())
                }
            };
            Ok(stream)
        })
    };
    Channel::new(
        "file-watcher",
        vec![
            ("watch", watch),
            ("unwatch", unwatch),
            ("disposeAll", dispose_all),
        ],
    )
    .with_events(vec![("onDynamicChange", change)])
}
