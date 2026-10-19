//! `escode-agent` channel 的 V4 面（docs/specs/rust-u2-agent-v4.md）：Renderer 对话主链路
//! （`packages/ui/src/v4/agentConversationTransport.ts`）经此订阅会话/列表/配置 topic、发命令、
//! 读分页与附件。每条 RPC 连接一个 [`scope::Scope`]（TS `createESCodeAgentConnectionScope`）。
//!
//! 未迁移的 `escode-agent` 方法（插件/自动化/MCP UI/wiki 等）不在方法表里，由 `session::respond`
//! 回显式 `NotImplemented`。
pub(crate) mod assembler;
pub(crate) mod base;
pub(crate) mod frames;
pub(crate) mod observe;
mod scope;
mod scope_handshake;
mod scope_methods;
mod scope_misc;
mod scope_routing;
mod scope_subscriptions;

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domains_task_support::{ReadContext, first_object};
use crate::session::{
    Channel, ChannelFactory, ConnectionContext, EventHandler, EventStream, MethodHandler, RpcError,
    handler_rpc,
};
use crate::wire::Wire;
use escode_host_runtime::{LifecycleEvent, RuntimeState};
use frames::TopicKind;
use scope::Scope;

/// Host 级共享句柄（V4 面与任务索引 syncer 共用同一 hub/实例表）。
pub(crate) fn shared_base(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
    index: Option<ReadContext>,
) -> Arc<base::Base> {
    Arc::new(base::Base::new(instances, options, index))
}

/// 任务域 Bot group 的 Host 内部观察器（独立帧泵；不建立 Renderer 连接）。
pub(crate) fn observer_base(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Arc<base::Base> {
    Arc::new(base::Base::new(instances, options, None))
}

/// 组合根：共享句柄 → 连接级 channel 工厂。
pub(crate) fn agent_channel_from_base(base: Arc<base::Base>) -> ChannelFactory {
    Arc::new(move |context: &ConnectionContext| connection_channel(&base, context))
}

fn connection_channel(base: &Arc<base::Base>, context: &ConnectionContext) -> Channel {
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
    let methods: Vec<(&'static str, MethodHandler)> = vec![
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
    let routed = |kind: TopicKind| -> EventHandler {
        let scope = Arc::clone(&scope);
        Arc::new(move |arg: Wire| Ok(scope.routed_event(kind, &first_object(arg))))
    };
    let events: Vec<(&'static str, EventHandler)> = vec![
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
    ];
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

/// 连接级后台任务：runtime 换代失效 owner；连接水位边沿转发给 runtime。
fn spawn_background(scope: &Arc<Scope>, base: &Arc<base::Base>, context: &ConnectionContext) {
    let mut lifecycle = base.instances.lifecycle();
    let weak = Arc::downgrade(scope);
    let lifecycle_task = tokio::spawn(async move {
        while let Some(event) = lifecycle.recv().await {
            let Some(scope) = weak.upgrade() else { return };
            // available 不是旧 ownership 的失效边界（冷启动的 subscribe 会先 pending、再 available、
            // 最后 ACK）；真正的换代一定先发布 unavailable。
            if event.state == RuntimeState::Unavailable {
                scope.invalidate_runtime(&event.identity, event.generation);
            }
        }
    });
    let mut flow = context.flow.clone();
    let weak = Arc::downgrade(scope);
    let flow_task = tokio::spawn(async move {
        while flow.changed().await.is_ok() {
            let state = *flow.borrow_and_update();
            let Some(scope) = weak.upgrade() else { return };
            scope.apply_flow(state).await;
        }
    });
    scope
        .tasks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend([lifecycle_task, flow_task]);
}

/// 静态事件：把实例表的生命周期流转成 TS 形状（`map` 返回 None 的事件不投递）。
fn lifecycle_event(
    base: &Arc<base::Base>,
    map: fn(&LifecycleEvent) -> Option<Value>,
) -> EventHandler {
    let base = Arc::clone(base);
    Arc::new(move |_arg: Wire| {
        let receiver = base.instances.lifecycle();
        let stream = futures_util::stream::unfold(receiver, move |mut receiver| async move {
            loop {
                let event = receiver.recv().await?;
                if let Some(value) = map(&event) {
                    return Some((Wire::Object(value), receiver));
                }
            }
        });
        Ok(Box::pin(stream) as EventStream)
    })
}

/// TS `ESCodeAgentRuntimeLifecycleEvent`。
fn lifecycle_payload(event: &LifecycleEvent) -> Option<Value> {
    let mut identity = json!({
        "generation": event.generation,
        "identity": event.runtime_identity,
        "workspaceKey": event.identity,
    });
    if let Some(pid) = event.process_id {
        identity["processId"] = json!(pid);
    }
    let mut payload = json!({
        "workspacePath": event.workspace_path,
        "workspaceKey": event.identity,
        "runtimeIdentity": identity,
        "state": match event.state {
            RuntimeState::Available => "available",
            RuntimeState::Unavailable => "unavailable",
        },
    });
    if let Some(workspace_identity) = event
        .workspace_identity
        .as_deref()
        .filter(|v| !v.is_empty())
    {
        payload["workspaceIdentity"] = Value::from(workspace_identity);
    }
    Some(payload)
}

/// TS `onRuntimeRestarted`：只在换代（generation > 1）且新实例已就位时发布 `{workspaceKey}`。
fn restarted_payload(event: &LifecycleEvent) -> Option<Value> {
    (event.state == RuntimeState::Available && event.generation > 1)
        .then(|| json!({"workspaceKey": event.identity}))
}
