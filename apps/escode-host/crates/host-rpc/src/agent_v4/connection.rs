//! `escode-agent` 连接级 channel 方法表（从 `mod.rs` 拆出以守住 400 行上限）。

use std::sync::Arc;

use futures_util::stream;
use serde_json::Value;

use crate::domains_task_support::first_object;
use crate::session::{
    Channel, ConnectionContext, EventHandler, MethodHandler, RpcError, handler, handler_rpc,
};
use crate::wire::Wire;

use super::automation_methods::automation_methods;
use super::base;
use super::frames::TopicKind;
use super::legacy_methods::{legacy_events, legacy_methods};
use super::scope::Scope;
use super::{lifecycle_event, lifecycle_payload, restarted_payload, spawn_background};

fn workspace_key(params: &Value) -> Option<String> {
    let path = params["workspacePath"].as_str()?;
    let identity = params["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Some(identity.unwrap_or(path).to_owned())
}

fn provider_headers_event(base: &Arc<base::Base>) -> EventHandler {
    let headers = base.provider_headers.clone();
    Arc::new(move |arg: Wire| {
        let Some(headers) = headers.clone() else {
            return Err("provider runtime headers need an App-level Host".to_owned());
        };
        let target = workspace_key(&first_object(arg));
        let receiver = headers.subscribe();
        let stream = stream::unfold(receiver, move |mut receiver| {
            let target = target.clone();
            async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            let matches = target.as_deref().is_none_or(|target| {
                                event["workspace"]["workspaceKey"].as_str() == Some(target)
                                    || event["workspace"]["workspacePath"].as_str() == Some(target)
                            });
                            if matches {
                                return Some((Wire::Object(event), receiver));
                            }
                        }
                        Err(_) => return None,
                    }
                }
            }
        });
        Ok(Box::pin(stream))
    })
}

pub(super) fn connection_channel(base: &Arc<base::Base>, context: &ConnectionContext) -> Channel {
    let scope = Scope::new(
        Arc::clone(base),
        context.connection_id.clone(),
        context.client_mode,
    );

    let query = |name: &'static str| -> MethodHandler {
        let scope = Arc::clone(&scope);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let scope = Arc::clone(&scope);
            Box::pin(async move { scope.query(name, &params).await.map(Wire::Object) })
        })
    };
    let attachment = |name: &'static str| -> MethodHandler {
        let scope = Arc::clone(&scope);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let scope = Arc::clone(&scope);
            Box::pin(async move {
                let result = scope.attachment(name, &params).await?;
                // `attachmentAbortV4` 是 `Promise<void>`。
                Ok(if result.is_null() {
                    Wire::Undefined
                } else {
                    Wire::Object(result)
                })
            })
        })
    };
    let mut methods: Vec<(&'static str, MethodHandler)> = vec![
        // `helloConversationV4()` 无实参：直接用作用域（不取 params）。
        (
            "helloConversationV4",
            bind(&scope, |s, _| async move { s.hello() }),
        ),
        (
            "initializeConversationV4",
            bind_unit(&scope, |s, hello| async move { s.initialize(&hello) }),
        ),
        (
            "subscribeConversationV4",
            bind(&scope, |s, p| async move {
                s.subscribe(TopicKind::Conversation, &p).await
            }),
        ),
        (
            "subscribeSessionsIndexV4",
            bind(&scope, |s, p| async move {
                s.subscribe(TopicKind::SessionsIndex, &p).await
            }),
        ),
        (
            "subscribeWorkspaceConfigV4",
            bind(&scope, |s, p| async move {
                s.subscribe(TopicKind::WorkspaceConfig, &p).await
            }),
        ),
        (
            "unsubscribeConversationV4",
            unsubscribe(&scope, TopicKind::Conversation),
        ),
        (
            "unsubscribeSessionsIndexV4",
            unsubscribe(&scope, TopicKind::SessionsIndex),
        ),
        (
            "unsubscribeWorkspaceConfigV4",
            unsubscribe(&scope, TopicKind::WorkspaceConfig),
        ),
        (
            "resyncConversationV4",
            bind(&scope, |s, p| async move {
                s.resync(TopicKind::Conversation, &p).await
            }),
        ),
        (
            "resyncSessionsIndexV4",
            bind(&scope, |s, p| async move {
                s.resync(TopicKind::SessionsIndex, &p).await
            }),
        ),
        (
            "resyncWorkspaceConfigV4",
            bind(&scope, |s, p| async move {
                s.resync(TopicKind::WorkspaceConfig, &p).await
            }),
        ),
        (
            "sendConversationCommandV4",
            bind(&scope, |s, p| async move { s.send_command(&p).await }),
        ),
        (
            "queryConversationCommandsV4",
            bind(&scope, |s, p| async move { s.query_commands(&p).await }),
        ),
        (
            "backgroundBashOutputV4",
            bind(
                &scope,
                |s, p| async move { s.background_bash_output(&p).await },
            ),
        ),
        (
            "setConnectionFlowStateV4",
            // terminal-client 不能自报水位（只有 trusted relay 可以；水位由传输层观测）。
            bind_unit(&scope, |s, _| async move {
                s.assert_open()?;
                Err(RpcError::new(
                    "Error",
                    "fault.connection.flowControlForbidden",
                ))
            }),
        ),
        (
            "attachmentPreviewSourceV4",
            bind(&scope, |s, p| async move { s.preview_source(&p).await }),
        ),
        ("conversationRowsRangeV4", query("conversationRowsRangeV4")),
        ("conversationPlansV4", query("conversationPlansV4")),
        (
            "conversationWorkflowRunEventsV4",
            query("conversationWorkflowRunEventsV4"),
        ),
        (
            "conversationWorkflowRunsV4",
            query("conversationWorkflowRunsV4"),
        ),
        (
            "conversationWorkflowRunArtifactsV4",
            query("conversationWorkflowRunArtifactsV4"),
        ),
        (
            "conversationWorkflowRunArtifactDataV4",
            query("conversationWorkflowRunArtifactDataV4"),
        ),
        (
            "conversationWorkflowRunArtifactReadV4",
            query("conversationWorkflowRunArtifactReadV4"),
        ),
        (
            "conversationWorkflowRunWorkspaceV4",
            query("conversationWorkflowRunWorkspaceV4"),
        ),
        (
            "conversationWorkflowRunNodeResultV4",
            query("conversationWorkflowRunNodeResultV4"),
        ),
        (
            "conversationFileChangesV4",
            query("conversationFileChangesV4"),
        ),
        (
            "conversationFileRewindPreviewV4",
            query("conversationFileRewindPreviewV4"),
        ),
        (
            "listSessionSubagents",
            bind(
                &scope,
                |s, p| async move { s.list_session_subagents(&p).await },
            ),
        ),
        (
            "getWorkspaceRuntimeIdentity",
            bind(&scope, |s, p| async move { s.runtime_identity(&p) }),
        ),
        (
            "respondProviderRuntimeHeaders",
            match base.provider_headers.clone() {
                Some(headers) => handler(move |arg: Wire| {
                    let headers = Arc::clone(&headers);
                    Box::pin(async move {
                        let params = first_object(arg);
                        let request_id = params["requestId"].as_str().unwrap_or_default();
                        headers
                            .respond(request_id, &params["response"])
                            .map(|_| Wire::Undefined)
                            .map_err(|message| ("Error".to_owned(), message))
                    })
                }),
                None => handler(|_arg: Wire| {
                    Box::pin(async move {
                        Err((
                            "NotImplemented".to_owned(),
                            "provider runtime headers need an App-level Host".to_owned(),
                        ))
                    })
                }),
            },
        ),
        (
            "listSessionSubagents",
            bind(
                &scope,
                |s, p| async move { s.list_session_subagents(&p).await },
            ),
        ),
        (
            "getWorkspaceRuntimeIdentity",
            bind(&scope, |s, p| async move { s.runtime_identity(&p) }),
        ),
        ("attachmentReadV4", query("attachmentReadV4")),
        (
            "conversationAttachmentReadV4",
            query("conversationAttachmentReadV4"),
        ),
        (
            "conversationAttachmentStatV4",
            query("conversationAttachmentStatV4"),
        ),
        ("attachmentBeginV4", attachment("attachmentBeginV4")),
        ("attachmentChunkV4", attachment("attachmentChunkV4")),
        ("attachmentCommitV4", attachment("attachmentCommitV4")),
        ("attachmentAbortV4", attachment("attachmentAbortV4")),
    ];
    methods.extend(automation_methods(
        base.index.as_ref().map(|read| read.path.clone()),
        base.automation.clone(),
    ));
    methods.extend(legacy_methods(base));
    let routed = |kind: TopicKind| -> EventHandler {
        let scope = Arc::clone(&scope);
        Arc::new(move |arg: Wire| Ok(scope.routed_event(kind, &first_object(arg))))
    };
    let mut events: Vec<(&'static str, EventHandler)> = vec![
        (
            "onDynamicConversationFrame",
            routed(TopicKind::Conversation),
        ),
        (
            "onDynamicSessionsIndexFrame",
            routed(TopicKind::SessionsIndex),
        ),
        (
            "onDynamicWorkspaceConfigFrame",
            routed(TopicKind::WorkspaceConfig),
        ),
        (
            "onAgentRuntimeLifecycle",
            lifecycle_event(base, lifecycle_payload),
        ),
        (
            "onAgentRuntimeRestarted",
            lifecycle_event(base, restarted_payload),
        ),
        (
            "onDynamicWorkspaceProviderRuntimeHeadersRequest",
            provider_headers_event(base),
        ),
    ];
    events.extend(legacy_events(base));
    let closing = Arc::clone(&scope);
    // 后台任务（换代失效、水位转发）只在真实连接上启动：探测方法表（契约对账/server-info）不能有副作用。
    let opening = (Arc::clone(&scope), Arc::clone(base), context.clone());
    Channel::new("escode-agent", methods)
        .with_events(events)
        .with_open(Arc::new(move || {
            spawn_background(&opening.0, &opening.1, &opening.2)
        }))
        .with_close(Arc::new(move || {
            let scope = Arc::clone(&closing);
            Box::pin(async move { scope.dispose().await })
        }))
}

/// 方法表项：作用域 + 第一个实参对象 → 对象结果。
fn bind<F, Fut>(scope: &Arc<Scope>, run: F) -> MethodHandler
where
    F: Fn(Arc<Scope>, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, RpcError>> + Send + 'static,
{
    let scope = Arc::clone(scope);
    handler_rpc(move |arg: Wire| {
        let future = run(Arc::clone(&scope), first_object(arg));
        Box::pin(async move { future.await.map(Wire::Object) })
    })
}

/// 同上，结果为 `Promise<void>`。
fn bind_unit<F, Fut>(scope: &Arc<Scope>, run: F) -> MethodHandler
where
    F: Fn(Arc<Scope>, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), RpcError>> + Send + 'static,
{
    let scope = Arc::clone(scope);
    handler_rpc(move |arg: Wire| {
        let future = run(Arc::clone(&scope), first_object(arg));
        Box::pin(async move { future.await.map(|()| Wire::Undefined) })
    })
}

fn unsubscribe(scope: &Arc<Scope>, kind: TopicKind) -> MethodHandler {
    let scope = Arc::clone(scope);
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let scope = Arc::clone(&scope);
        Box::pin(async move {
            scope
                .unsubscribe(kind, &params)
                .await
                .map(|()| Wire::Undefined)
        })
    })
}
