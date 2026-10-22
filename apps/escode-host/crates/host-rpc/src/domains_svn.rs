//! `svn` channel（TS `ServiceChannels.Svn` = `"svn"`；docs/svn/architecture.md）。
//!
//! 方法表即分派；业务实现全部在 `escode_host_services::svn::SvnService`（Host 内唯一实例）。
//! 可预期的业务失败已经以 `{ ok: false, issue, message }` 返回；这里只负责参数解包与
//! 进程级错误整形（TS 抛 `Error` → `("Error", message)`）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use escode_host_services::svn::SvnService;
use serde_json::Value;

use crate::domains_task_support::first_object;
use crate::session::{Channel, MethodHandler, handler};
use crate::wire::Wire;

type SvcFuture = Pin<Box<dyn Future<Output = Value> + Send>>;

fn value_method(
    service: Arc<SvnService>,
    run: fn(Arc<SvnService>, Value) -> SvcFuture,
) -> MethodHandler {
    handler(move |arg: Wire| {
        let service = Arc::clone(&service);
        let params = first_object(arg);
        Box::pin(async move { Ok(Wire::Object(run(service, params).await)) })
    })
}

/// 组合根构造 `svn` channel：进程 env 经 `core_api::env` 唯一出口取得。
pub fn svn_channel(
    home: std::path::PathBuf,
    provider: Option<Arc<escode_host_services::provider::ProviderService>>,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    let service = Arc::new(SvnService::new(
        &escode_cli_core_api::env::vars(),
        home,
        escode_cli_core_api::env::temp_dir(),
    ));
    let generate = crate::domains_svn_commit_message::generate_commit_message_method(
        Arc::clone(&service),
        provider,
        instances,
        options,
    );
    let set_binary = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                let params = first_object(arg);
                service
                    .set_binary_path(&params)
                    .await
                    .map(|()| Wire::Undefined)
                    .map_err(|error| ("Error".to_owned(), format!("{error:#}")))
            })
        })
    };
    let methods: Vec<(&'static str, MethodHandler)> = vec![
        (
            "getWorkingCopyInfo",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_working_copy_info(&params).await })
            }),
        ),
        (
            "getChanges",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_changes(&params).await })
            }),
        ),
        (
            "getDiff",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_diff(&params).await })
            }),
        ),
        (
            "getLog",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_log(&params).await })
            }),
        ),
        (
            "getFileLog",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_file_log(&params).await })
            }),
        ),
        (
            "getBlame",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_blame(&params).await })
            }),
        ),
        (
            "getRemoteStatus",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.get_remote_status(&params).await })
            }),
        ),
        (
            "listBranches",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.list_branches(&params).await })
            }),
        ),
        (
            "commit",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.commit(&params).await })
            }),
        ),
        (
            "update",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.update(&params).await })
            }),
        ),
        (
            "revert",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.revert(&params).await })
            }),
        ),
        (
            "resolve",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.resolve(&params).await })
            }),
        ),
        (
            "switchBranch",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.switch_branch(&params).await })
            }),
        ),
        (
            "merge",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.merge(&params).await })
            }),
        ),
        (
            "cleanup",
            value_method(Arc::clone(&service), |service, params| {
                Box::pin(async move { service.cleanup(&params).await })
            }),
        ),
        ("setBinaryPath", set_binary),
        ("generateCommitMessage", generate),
    ];
    Channel::new("svn", methods)
}
