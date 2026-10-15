//! `file` channel 的工作区索引 / 搜索 / `.escodeignore` 方法（docs/specs/rust-u2-file-index.md）。

use std::sync::Arc;

use escode_host_services::file_index::{FileIndexService, LocaleCompare};
use serde_json::Value;

use crate::domains_task_support::first_object;
use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

fn text(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn number(params: &Value, key: &str) -> f64 {
    params.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn error(error: anyhow::Error) -> (String, String) {
    ("Error".to_owned(), format!("{error:#}"))
}

type IndexFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Wire, anyhow::Error>> + Send>>;

/// 6 个方法的处理函数（与 `file` channel 其余方法合并成一张表）。
pub fn index_methods() -> Vec<(&'static str, MethodHandler)> {
    let compare: LocaleCompare = Arc::new(crate::collation::compare_names);
    let service = Arc::new(FileIndexService::new(compare));
    let method = |run: fn(Arc<FileIndexService>, Value) -> IndexFuture| {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let future = run(Arc::clone(&service), first_object(arg));
            Box::pin(async move { future.await.map_err(error) })
        })
    };
    vec![
        (
            "searchWorkspaceFiles",
            method(|svc, params| {
                Box::pin(async move {
                    // 非字符串 query、非数字 limit（含 NaN/Infinity）都归为同一个参数错误（TS 同文案）。
                    let Some(query) = params.get("query").and_then(Value::as_str) else {
                        anyhow::bail!("Invalid workspace file search query or limit");
                    };
                    let limit = match params.get("limit") {
                        None | Some(Value::Null) => None,
                        Some(value) => Some(value.as_f64().unwrap_or(f64::NAN)),
                    };
                    let identity = params.get("workspaceIdentity").and_then(Value::as_str);
                    let refresh = params
                        .get("refresh")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    svc.search(&text(&params, "rootPath"), identity, query, limit, refresh)
                        .await
                        .map(Wire::Object)
                })
            }),
        ),
        (
            "listWorkspaceFilesLength",
            method(|svc, params| {
                Box::pin(async move {
                    svc.length(&text(&params, "rootPath"))
                        .await
                        .map(|length| Wire::Int(length as i64))
                })
            }),
        ),
        (
            "listWorkspaceFilesRange",
            method(|svc, params| {
                Box::pin(async move {
                    svc.range(
                        &text(&params, "rootPath"),
                        number(&params, "offset"),
                        number(&params, "length"),
                    )
                    .await
                    .map(Wire::Str)
                })
            }),
        ),
        (
            "readWorkspaceFileSearchIgnore",
            method(|svc, params| {
                Box::pin(async move {
                    svc.read_ignore(&text(&params, "rootPath"))
                        .await
                        .map(Wire::Object)
                })
            }),
        ),
        (
            "applyWorkspaceFileSearchIgnoreTransform",
            method(|svc, params| {
                Box::pin(async move {
                    svc.transform_ignore(&text(&params, "rootPath"), &text(&params, "transform"))
                        .await
                        .map(Wire::Object)
                })
            }),
        ),
        (
            "writeWorkspaceFileSearchIgnore",
            method(|svc, params| {
                Box::pin(async move {
                    svc.write_ignore(&text(&params, "rootPath"), &text(&params, "content"))
                        .await
                        .map(|()| Wire::Undefined)
                })
            }),
        ),
    ]
}
