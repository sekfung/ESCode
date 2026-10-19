//! `off-peak-task` channel（TS `ServiceChannels.OffPeakTask`；docs/specs/rust-u2-off-peak.md）。
//!
//! 当前实现 Host 自足的本地管理面；取号/额度/创建仍是显式缺口（方法表不登记，
//! Gateway 回 `NotImplemented` 并列出已实现方法）。

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::off_peak::OffPeakService;
use serde_json::Value;

use crate::session::{Channel, MethodHandler, handler};
use crate::wire::Wire;

fn args(arg: &Wire) -> Vec<Wire> {
    match arg {
        Wire::Array(items) => items.clone(),
        Wire::Undefined => Vec::new(),
        other => vec![other.clone()],
    }
}

fn id_arg(arg: &Wire) -> Result<String, (String, String)> {
    args(arg)
        .first()
        .and_then(Wire::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            (
                "InvalidRequest".to_owned(),
                "off-peak-task requires a task id".to_owned(),
            )
        })
}

fn object_arg(arg: &Wire, index: usize) -> Value {
    args(arg)
        .get(index)
        .cloned()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

fn gap() -> MethodHandler {
    handler(|_arg: Wire| {
        Box::pin(async move {
            Err((
                "NotImplemented".to_owned(),
                "off-peak-task needs dataBaseDir: Main must pass dataBaseDir in rpcListen."
                    .to_owned(),
            ))
        })
    })
}

fn by_id<F, Fut>(service: Option<Arc<OffPeakService>>, run: F) -> MethodHandler
where
    F: Fn(Arc<OffPeakService>, String) -> Fut + Send + Sync + Copy + 'static,
    Fut: Future<Output = Result<Value, String>> + Send + 'static,
{
    let Some(service) = service else {
        return gap();
    };
    handler(move |arg: Wire| {
        let id = id_arg(&arg);
        let service = Arc::clone(&service);
        let future = id.map(|id| run(service, id));
        Box::pin(async move {
            let result = future?
                .await
                .map_err(|message| ("Error".to_owned(), message))?;
            Ok(Wire::Object(result))
        })
    })
}

pub(crate) fn off_peak_channel(data_base_dir: Option<PathBuf>) -> Channel {
    let service = data_base_dir.map(|dir| Arc::new(OffPeakService::new(dir)));
    let list = match service.clone() {
        Some(service) => handler(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .list()
                    .await
                    .map(Wire::Object)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        }),
        None => gap(),
    };
    let delete = match service.clone() {
        Some(service) => handler(move |arg: Wire| {
            let (service, id) = (Arc::clone(&service), id_arg(&arg));
            Box::pin(async move {
                service
                    .delete(id?)
                    .await
                    .map(|_| Wire::Undefined)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        }),
        None => gap(),
    };
    let update = match service.clone() {
        Some(service) => handler(move |arg: Wire| {
            let (service, id, params) = (Arc::clone(&service), id_arg(&arg), object_arg(&arg, 1));
            Box::pin(async move {
                service
                    .update(id?, params)
                    .await
                    .map(Wire::Object)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        }),
        None => gap(),
    };

    Channel::new(
        "off-peak-task",
        vec![
            ("list", list),
            (
                "get",
                by_id(service.clone(), |s, id| async move { s.get(id).await }),
            ),
            (
                "cancelTask",
                by_id(service.clone(), |s, id| async move { s.cancel(id).await }),
            ),
            (
                "pauseTask",
                by_id(service.clone(), |s, id| async move { s.pause(id).await }),
            ),
            (
                "continueTask",
                by_id(service.clone(), |s, id| async move { s.resume(id).await }),
            ),
            (
                "deleteHistory",
                by_id(service, |s, id| async move { s.delete_history(id).await }),
            ),
            ("deleteTask", delete),
            ("updateTask", update),
        ],
    )
}
