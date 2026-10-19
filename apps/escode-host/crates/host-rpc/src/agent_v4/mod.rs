//! `escode-agent` channel 的 V4 面（docs/specs/rust-u2-agent-v4.md）：Renderer 对话主链路
//! （`packages/ui/src/v4/agentConversationTransport.ts`）经此订阅会话/列表/配置 topic、发命令、
//! 读分页与附件。每条 RPC 连接一个 [`scope::Scope`]（TS `createESCodeAgentConnectionScope`）。
//!
//! 未迁移的 `escode-agent` 方法（插件/自动化/MCP UI/wiki 等）不在方法表里，由 `session::respond`
//! 回显式 `NotImplemented`。
pub(crate) mod assembler;
mod automation_methods;
pub(crate) mod base;
mod connection;
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

use crate::domains_task_support::ReadContext;
use crate::session::{ChannelFactory, ConnectionContext, EventHandler, EventStream};
use crate::wire::Wire;
use connection::connection_channel;
use escode_host_runtime::{LifecycleEvent, RuntimeState};
use scope::Scope;

/// Host 级共享句柄（V4 面与任务索引 syncer 共用同一 hub/实例表）。
pub(crate) fn shared_base(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
    index: Option<ReadContext>,
    provider_headers: Option<Arc<crate::provider_headers::ProviderHeaders>>,
) -> Arc<base::Base> {
    Arc::new(base::Base::new(instances, options, index, provider_headers))
}

/// 任务域 Bot group 的 Host 内部观察器（独立帧泵；不建立 Renderer 连接）。
pub(crate) fn observer_base(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Arc<base::Base> {
    Arc::new(base::Base::new(instances, options, None, None))
}

/// 组合根：共享句柄 → 连接级 channel 工厂。
pub(crate) fn agent_channel_from_base(base: Arc<base::Base>) -> ChannelFactory {
    Arc::new(move |context: &ConnectionContext| connection_channel(&base, context))
}

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
