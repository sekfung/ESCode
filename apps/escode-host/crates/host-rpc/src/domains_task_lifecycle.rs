//! `escode-task` 生命周期（S5）：`closeTask` / `resumeTask`。
//!
//! 两个入口都复用 session 通道的运行时调用与索引同步：`closeTask` 走 `session/close` +
//! `updateIndexedTaskState(deleted)`；`resumeTask` 复用 [`resume_snapshot`]（thoughtLevel 重放）+
//! `write_index`（reason `task_status_changed`）。

use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_services::session_params::{
    append_workspace_to_filesystem_mcp_servers, session_forward_params,
};
use escode_host_services::task_projection::{IndexOverrides, index_meta_from_snapshot};
use escode_host_services::tasks_index::SyncOptions;
use escode_host_services::tasks_index::write_state::StatePatch;

use crate::deferred_drafts::DeferredDrafts;
use crate::domains_session_write::{SessionWriteContext, resume_snapshot, write_index};
use crate::domains_task_state::{StateContext, Task, method, update_indexed};
use crate::domains_task_support::context_ref;
use crate::forward::{self, Extract, Shaped};
use crate::session::{MethodHandler, RpcError};
use crate::task_targets::TaskTarget;
use crate::wire::Wire;
use crate::workspace_events::Target;

pub(super) fn lifecycle_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("closeTask", close_task(context)),
        ("resumeTask", resume_task(context)),
    ]
}

/// TS `closeTask`：`session/close` → 索引 `deleted` → workspace 级 `task_meta_changed`。
fn close_task(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
        if task_id.is_empty() {
            return Err(RpcError::new("InvalidRequest", "taskId is required."));
        }
        let target = context
            .read
            .targets
            .get(&task_id)
            .map_err(|(code, message)| RpcError::new(code, message))?;
        let Some(runtime) = context.read.runtime.clone() else {
            return Err(RpcError::new(
                "InvalidRequest",
                "Host has no runtime access.",
            ));
        };
        let (method, body) = session_forward_params("closeSession", &json!({"sessionId": task_id}))
            .ok_or_else(|| RpcError::new("Fatal", "no runtime mapping for closeSession"))?;
        let shaped = Shaped {
            method,
            body,
            with_workspace: false,
            trace: None,
        };
        let routing = json!({
            "workspacePath": target.workspace_path,
            "workspaceIdentity": target.workspace_identity,
        });
        // TS 忽略 close 的返回值（`.then(() => undefined)`）。
        let _ = forward::runtime_call_shaped(
            &runtime.instances,
            &runtime.options,
            routing,
            shaped,
            Extract::Result,
        )
        .await?;
        let task = Task {
            workspace_path: target.workspace_path.clone(),
            workspace_identity: target.workspace_identity.clone(),
            task_id: task_id.clone(),
        };
        update_indexed(
            &context,
            &task,
            StatePatch {
                deleted: Some(true),
                ..StatePatch::default()
            },
        )
        .await?;
        context.events.task_list_changed(
            &Target {
                workspace_path: &target.workspace_path,
                workspace_identity: target.workspace_identity.as_deref(),
                task_id: Some(&task_id),
            },
            None,
            "task_meta_changed",
            None,
        );
        Ok(Wire::Undefined)
    })
}

/// TS `resumeTask`：索引 hint（thoughtLevel）→ `session/resume`（复用 session 通道）→ 索引收敛。
fn resume_task(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        resume_task_impl(context, params).await
    })
}

pub(crate) async fn resume_task_impl(
    context: StateContext,
    params: Value,
) -> crate::session::RpcResult {
    {
        let workspace_path = params["workspacePath"]
            .as_str()
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| RpcError::new("InvalidRequest", "workspacePath is required."))?;
        let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
        if task_id.is_empty() {
            return Err(RpcError::new("InvalidRequest", "taskId is required."));
        }
        let Some(runtime) = context.read.runtime.clone() else {
            return Err(RpcError::new(
                "InvalidRequest",
                "Host has no runtime access.",
            ));
        };
        let write = SessionWriteContext {
            runtime,
            read: Some(context.read.clone()),
            drafts: Arc::new(DeferredDrafts::new()),
            events: Arc::clone(&context.events),
        };
        let mut injected = params.clone();
        // session 转发层以 `sessionId` 为键；任务域入参是 `taskId`。
        injected["sessionId"] = Value::from(task_id.clone());
        // 显式 thoughtLevel 优先；缺失时回填索引里的 task-local 值（TS `resolveTaskIndexResumeHints`）。
        let missing_thought = injected["thoughtLevel"]
            .as_str()
            .map(str::trim)
            .is_none_or(str::is_empty);
        if missing_thought {
            let (path, identity, id) = (
                workspace_path.clone(),
                injected["workspaceIdentity"].as_str().map(str::to_owned),
                task_id.clone(),
            );
            // 索引读失败不阻断恢复（TS 记 warn 后继续）。
            let meta = context_ref(&context.read)
                .run(move |index| {
                    index
                        .get_task_meta(&path, identity.as_deref(), &id)
                        .map_err(|error| error.to_string())
                })
                .await;
            if let Ok(Some(meta)) = meta
                && let Some(level) = meta["thoughtLevel"]
                    .as_str()
                    .map(str::trim)
                    .filter(|level| !level.is_empty())
            {
                injected["thoughtLevel"] = Value::from(level.to_owned());
            }
        }
        if let Some(servers) =
            append_workspace_to_filesystem_mcp_servers(injected.get("mcpServers"), &workspace_path)
        {
            injected["mcpServers"] = servers;
        }
        let (snapshot, overrides) = resume_snapshot(&write, &injected, &injected).await?;
        write_index(
            &write,
            &injected,
            &snapshot,
            overrides.clone(),
            "task_status_changed",
        )
        .await;
        let automation = injected["automationId"].as_str().map(str::to_owned);
        let identity = injected["workspaceIdentity"].as_str().map(str::to_owned);
        let meta = finish_meta(&context, &injected, &snapshot, &overrides).await?;
        context.read.targets.remember(TaskTarget {
            task_id,
            workspace_path,
            workspace_identity: identity,
            cron_automation_id: automation,
            remote_session_id: None,
        });
        Ok(Wire::Object(meta))
    }
}

/// TS `syncTaskIndexSnapshot` + automation 归属回填。
async fn finish_meta(
    context: &StateContext,
    params: &Value,
    snapshot: &Value,
    overrides: &IndexOverrides,
) -> Result<Value, RpcError> {
    let workspace_path = params["workspacePath"].as_str().unwrap_or_default();
    let task_id = params["taskId"].as_str().unwrap_or_default();
    let workspace_identity = params["workspaceIdentity"].as_str();
    let automation = params["automationId"].as_str().map(str::to_owned);
    if automation.is_none() {
        let (path, identity, id) = (
            workspace_path.to_owned(),
            workspace_identity.map(str::to_owned),
            task_id.to_owned(),
        );
        let meta = context_ref(&context.read)
            .run(move |index| {
                index
                    .get_task_meta(&path, identity.as_deref(), &id)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(RpcError::from)?;
        return Ok(meta.unwrap_or_else(|| index_meta_from_snapshot(snapshot, overrides)));
    }
    let mut meta = index_meta_from_snapshot(snapshot, overrides);
    if let Some(automation) = automation {
        meta["cronAutomationId"] = Value::from(automation);
    }
    let object = meta.as_object().cloned().unwrap_or_default();
    context_ref(&context.read)
        .run(move |index| {
            index
                .sync_task_meta(&object, &SyncOptions::default())
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(RpcError::from)
}
