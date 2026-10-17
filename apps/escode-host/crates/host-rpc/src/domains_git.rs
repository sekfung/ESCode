//! `git` channel（TS `ServiceChannels.Git` = `"git"`；docs/specs/rust-u2-git.md）。
//!
//! 方法表即分派；`generateCommitMessage` 由 `domains_git_commit_message` 提供（spec §7）。
//! 业务实现全部在 `escode_host_services::git::GitService`（Host 内唯一实例）；这里只做参数解包与
//! 错误整形（TS 抛 `Error` → `("Error", message)`）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use escode_host_services::git::{GitService, LocaleCompare};
use serde_json::Value;

use crate::domains_task_support::first_object;
use crate::session::{Channel, ChannelResult, MethodHandler, handler};
use crate::wire::Wire;

type GitFuture = Pin<Box<dyn Future<Output = anyhow::Result<Option<Value>>> + Send>>;

/// 组合根构造 `git` channel：进程 env 经 `core_api::env` 唯一出口取得。
/// `generateCommitMessage` 需要 Host 的当前模型（Provider 视图）与运行时实例。
pub fn git_channel(
    service: Arc<GitService>,
    provider: Option<Arc<escode_host_services::provider::ProviderService>>,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    let generate = crate::domains_git_commit_message::generate_commit_message_method(
        Arc::clone(&service),
        provider,
        instances,
        options,
    );
    git_channel_with(service, vec![("generateCommitMessage", generate)])
}

/// 组合根构造 Host 内**唯一**的 `GitService`：`git` 与 `git-checkpoint` 两个通道共用同一实例
/// （二进制探测结果与 `collapsed` 折叠状态因此只有一份，符合单所有者）。
pub fn git_service() -> Arc<GitService> {
    let locale_compare: LocaleCompare = Arc::new(crate::collation::compare_names);
    Arc::new(GitService::new(
        &escode_cli_core_api::env::vars(),
        escode_cli_core_api::env::temp_dir(),
        locale_compare,
    ))
}

pub fn git_channel_with(
    service: Arc<GitService>,
    extra: Vec<(&'static str, MethodHandler)>,
) -> Channel {
    let method = |run: fn(Arc<GitService>, Value) -> GitFuture| -> MethodHandler {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let future = run(Arc::clone(&service), first_object(arg));
            Box::pin(async move { to_channel_result(future.await) })
        })
    };
    let mut methods = vec![
        (
            "getRepositorySummary",
            method(|git, params| {
                Box::pin(async move {
                    git.repository_summary(&text(&params, "workspacePath"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "getWorkspaceRepositoryInfo",
            method(|git, params| {
                Box::pin(async move {
                    git.workspace_repository_info(&text(&params, "workspacePath"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "getLocalBranches",
            method(|git, params| {
                Box::pin(async move {
                    git.local_branches(&text(&params, "workspacePath"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "getCommitGraph",
            method(|git, params| {
                Box::pin(async move {
                    let max_count = params.get("maxCount").and_then(Value::as_f64);
                    let skip = params.get("skip").and_then(Value::as_f64);
                    git.commit_graph(&text(&params, "workspacePath"), max_count, skip)
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "switchBranch",
            method(|git, params| {
                Box::pin(async move {
                    git.switch_branch(
                        &text(&params, "workspacePath"),
                        &text(&params, "targetBranchName"),
                    )
                    .await
                    .map(Some)
                })
            }),
        ),
        (
            "createBranchAndSwitch",
            method(|git, params| {
                Box::pin(async move {
                    let start_point = params
                        .get("startPoint")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    git.create_branch_and_switch(
                        &text(&params, "workspacePath"),
                        &text(&params, "branchName"),
                        start_point.as_deref(),
                    )
                    .await
                    .map(Some)
                })
            }),
        ),
        (
            "getChanges",
            method(|git, params| {
                Box::pin(async move {
                    git.changes(&text(&params, "workspacePath"), &text(&params, "sourceId"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "getIgnoredPaths",
            method(|git, params| {
                Box::pin(async move {
                    git.ignored_paths(&text(&params, "workspacePath"), &strings(&params, "paths"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "getDiff",
            method(|git, params| Box::pin(async move { git.diff(&params).await.map(Some) })),
        ),
        (
            "getBranchComparison",
            method(|git, params| {
                Box::pin(async move {
                    git.branch_comparison(&text(&params, "workspacePath"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "stagePaths",
            method(|git, params| {
                Box::pin(async move {
                    git.stage(&text(&params, "workspacePath"), &strings(&params, "paths"))
                        .await
                        .map(|()| None)
                })
            }),
        ),
        (
            "unstagePaths",
            method(|git, params| {
                Box::pin(async move {
                    git.unstage(&text(&params, "workspacePath"), &strings(&params, "paths"))
                        .await
                        .map(|()| None)
                })
            }),
        ),
        (
            "discardPaths",
            method(|git, params| {
                Box::pin(async move {
                    let staged = params
                        .get("staged")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    git.discard(
                        &text(&params, "workspacePath"),
                        &strings(&params, "paths"),
                        staged,
                    )
                    .await
                    .map(|()| None)
                })
            }),
        ),
        (
            "commit",
            method(|git, params| {
                Box::pin(async move {
                    let paths = params
                        .get("paths")
                        .is_some_and(Value::is_array)
                        .then(|| strings(&params, "paths"));
                    let staged_only = params
                        .get("stagedOnly")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    git.commit(
                        &text(&params, "workspacePath"),
                        &text(&params, "message"),
                        paths.as_deref(),
                        staged_only,
                    )
                    .await
                    .map(Some)
                })
            }),
        ),
        (
            "push",
            method(|git, params| {
                Box::pin(async move { git.push(&text(&params, "workspacePath")).await.map(Some) })
            }),
        ),
        (
            "getIdentity",
            method(|git, params| {
                Box::pin(async move {
                    git.identity(&text(&params, "workspacePath"))
                        .await
                        .map(Some)
                })
            }),
        ),
        (
            "refresh",
            method(|git, params| {
                Box::pin(async move {
                    let flag =
                        |key: &str| params.get(key).and_then(Value::as_bool).unwrap_or(false);
                    git.refresh(
                        &text(&params, "workspacePath"),
                        flag("includeIdentity"),
                        flag("includeBranchComparison"),
                    )
                    .await
                    .map(Some)
                })
            }),
        ),
    ];
    methods.extend(extra);
    Channel::new("git", methods)
}

fn text(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn strings(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// TS 方法抛 `Error` → 客户端收到 `name="Error"`、`message` 原文；`void` 方法回 `undefined`。
fn to_channel_result(result: anyhow::Result<Option<Value>>) -> ChannelResult {
    match result {
        Ok(Some(value)) => Ok(Wire::Object(value)),
        Ok(None) => Ok(Wire::Undefined),
        Err(error) => Err(("Error".to_owned(), error.to_string())),
    }
}
