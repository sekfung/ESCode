//! `escode-session` 的写入口与带 Host 职责的转发（TS `escodeSessionService.ts`）。
//!
//! | 方法 | Host 职责（TS 同序） |
//! | --- | --- |
//! | `createSession` | MCP 注入 → 整形 → `session/create`（信封带 `trace.traceId`）→ deferred 只记草稿、原样返回；否则 `withApiRetryRuntime` + 索引写 |
//! | `resumeSession` | MCP 注入 → 整形 → `session/resume` → `withApiRetryRuntime` → thoughtLevel 重放 → 索引写（`broadcastSnapshot !== false`） |
//! | `setModel` | 草稿判定 → `session/setModel` → `withApiRetryRuntime` → 非草稿写索引（`modelOverride`） |
//! | `closeSession` | 清草稿 → `session/close` → 回 `undefined`（TS `.then(() => undefined)`） |
//! | `closeDeferredDraftSession` | 条件关闭（`expectedPersistence: "deferred"`），成功才清草稿；失败回 `false` |
//! | `promoteDeferredDraftSession` | 清草稿标记，回 `undefined` |
//!
//! 索引写失败只记日志、不让调用失败（TS `broadcastSnapshot` 吞错只 warn；评审 A12：原实现把
//! `TasksIndexUnavailable` 抛给调用方）。工具面灰度一律 Host 判定（[`ToolGates`]，当前关闭）。
//! 登记差异见 `docs/specs/rust-u2-tasks-index.md` §6.5。

use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_services::session_params::{
    ToolGates, append_workspace_to_filesystem_mcp_servers, current_thought_level,
    replayable_thought_level, session_create_params, session_forward_params, session_resume_params,
    with_api_retry_runtime,
};
use escode_host_services::task_projection::{
    IndexOverrides, format_model_picker_value, has_user_visible_content, sync_snapshot_to_index,
};

use super::deferred_drafts::DeferredDrafts;
use super::domains_session_model::sync_task_model;
use super::domains_task_support::{
    ReadContext, RuntimeAccess, context_ref, first_object, workspace_identity_of, workspace_path_of,
};
use crate::forward::{Extract, Shaped, runtime_call_shaped};
use crate::session::{MethodHandler, RpcError, RpcResult, handler_rpc};
use crate::task_targets::remember_from_meta;
use crate::wire::Wire;

/// session 域上下文：运行时转发 + 索引侧（`dataBaseDir` 缺省时为 None）+ 草稿注册表。
///
/// 只有写索引的方法（create/resume/setModel）依赖索引侧；缺 `dataBaseDir` 时它们回显式缺口，
/// 其余转发照常工作。
#[derive(Clone)]
pub(super) struct SessionWriteContext {
    pub runtime: RuntimeAccess,
    pub read: Option<ReadContext>,
    pub drafts: Arc<DeferredDrafts>,
    /// 写索引后广播 `workspace_task_list_changed`（与任务域共用一条总线）。
    pub events: Arc<crate::workspace_events::WorkspaceEvents>,
}

type Error = RpcError;

fn require_workspace(params: &Value) -> Result<String, Error> {
    let workspace_path = workspace_path_of(params);
    if workspace_path.is_empty() {
        return Err(RpcError::new(
            "InvalidRequest",
            "workspacePath is required.",
        ));
    }
    Ok(workspace_path)
}

/// `escode-session.createSession`。
pub(super) fn create_session_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            let workspace_path = require_workspace(&params)?;
            // TS `params.sessionTraceId ?? createSessionTraceId()`（UUID v4），随信封 `trace` 下发。
            let trace_id = params["sessionTraceId"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let injected = inject_filesystem_workspace(&params, &workspace_path);
            let shaped = Shaped {
                method: "session/create",
                body: session_create_params(&injected, &ToolGates::default()),
                with_workspace: true,
                trace: Some(json!({"traceId": trace_id})),
            };
            let snapshot = call(&context, &params, shaped).await?;
            if injected["persistence"] == "deferred" {
                // 空草稿不进索引：只记注册表（TS 此分支返回 agent 原始快照，不经 withApiRetryRuntime）。
                context.drafts.remember(&injected, &snapshot);
                return Ok(Wire::Object(snapshot));
            }
            let snapshot = with_api_retry_runtime(snapshot);
            // 首发广播沿用 task_meta_changed（TS：一个任务一次的低频事件）。
            write_index(
                &context,
                &injected,
                &snapshot,
                IndexOverrides::default(),
                "task_meta_changed",
            )
            .await;
            Ok(Wire::Object(snapshot))
        })
    })
}

/// `escode-session.resumeSession`。
pub(super) fn resume_session_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            let workspace_path = require_workspace(&params)?;
            // TS `const { broadcastSnapshot = true, ...resumeParams } = params`。
            let broadcast = params["broadcastSnapshot"] != false;
            let injected = inject_filesystem_workspace(&params, &workspace_path);
            let shaped = Shaped {
                method: "session/resume",
                body: session_resume_params(&injected, &ToolGates::default()),
                with_workspace: true,
                trace: None,
            };
            let mut snapshot = with_api_retry_runtime(call(&context, &params, shaped).await?);
            let thought_override = replayable_thought_level(&params, &snapshot);
            if let Some(level) = &thought_override
                && current_thought_level(&snapshot) != Some(level.as_str())
            {
                // 打开历史任务时快照可能带着同 workspace 草稿态的思考强度：对同一会话显式重放
                // task-local thoughtLevel，再用修正后的快照写索引（TS `resumeSession` 同序）。
                let shaped = Shaped {
                    method: "session/setThoughtLevel",
                    body: json!({"sessionId": params["sessionId"], "thoughtLevel": level}),
                    with_workspace: false,
                    trace: None,
                };
                snapshot = with_api_retry_runtime(call(&context, &params, shaped).await?);
            }
            let model_override = params
                .get("model")
                .filter(|model| model.is_object())
                .map(|model| format_model_picker_value(Some(model)));
            if broadcast {
                let overrides = IndexOverrides {
                    model: model_override.clone(),
                    thought_level: thought_override,
                };
                // 打开/恢复是快照收敛，不改归属：task_status_changed（缺省 task_meta_changed 会让每次点开任务
                // 都触发全局 membership 重拉）。
                write_index(
                    &context,
                    &injected,
                    &snapshot,
                    overrides,
                    "task_status_changed",
                )
                .await;
            }
            // `broadcastSnapshot: false` + model 时 TS 只改索引的 model 列（`syncTaskModel` →
            // `updateTaskState`）：发送前 resume 关闭快照广播避免旧终态覆盖 streaming UI，
            // 但 sqlite 的 model 仍要从已删除的历史模型更新到本次实际使用的可用模型。
            if !broadcast {
                sync_task_model(&context, &injected, &params, model_override.as_deref()).await;
            }
            Ok(Wire::Object(snapshot))
        })
    })
}

/// `escode-session.setModel`。
pub(super) fn set_model_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            let is_draft = is_draft(&context, &params);
            let snapshot = with_api_retry_runtime(forward(&context, "setModel", &params).await?);
            if is_draft {
                // 草稿只存在于 runtime 内存：写索引会留下进程重启后无法 resume 的脏会话。
                return Ok(Wire::Object(snapshot));
            }
            let overrides = IndexOverrides {
                model: Some(format_model_picker_value(params.get("model"))),
                thought_level: None,
            };
            // 切模型是纯配置变更：task_model_changed（不得落到 task_meta_changed 触发列表整刷）。
            write_index(
                &context,
                &params,
                &snapshot,
                overrides,
                "task_model_changed",
            )
            .await;
            Ok(Wire::Object(snapshot))
        })
    })
}

/// 只做 `withApiRetryRuntime` 的转发（`readSession` / `setThoughtLevel` / `setMode`）。
pub(super) fn api_retry_forward_handler(
    context: SessionWriteContext,
    method: &'static str,
) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            let snapshot = forward(&context, method, &params).await?;
            Ok(Wire::Object(with_api_retry_runtime(snapshot)))
        })
    })
}

/// `escode-session.closeSession`。
pub(super) fn close_session_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            forget_draft(&context, &params);
            forward(&context, "closeSession", &params).await?;
            Ok(Wire::Undefined)
        })
    })
}

/// `escode-session.closeDeferredDraftSession`。
pub(super) fn close_deferred_draft_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            let mut conditional = params.clone();
            conditional["expectedPersistence"] = Value::from("deferred");
            // 失败（含旧 Agent 拒绝 expectedPersistence）回 false：保留旧 session 并让 UI 新建草稿，
            // 不能退回无条件 close（可能关掉刚被其他客户端提升的 active task）。
            let closed = match forward(&context, "closeSession", &conditional).await {
                Ok(result) => result["closed"].as_bool().unwrap_or(true),
                Err(_) => false,
            };
            if closed {
                forget_draft(&context, &params);
            }
            Ok(Wire::Object(Value::Bool(closed)))
        })
    })
}

/// `escode-session.promoteDeferredDraftSession`。
pub(super) fn promote_deferred_draft_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            // 清除草稿标记：此后该会话的状态变更不再被挡在索引之外（TS 另通知 syncer 建订阅，
            // 事件面落地前由下一次写入口/快照读写回索引）。
            forget_draft(&context, &params);
            Ok(Wire::Undefined)
        })
    })
}

fn draft_key(params: &Value) -> (String, Option<String>, String) {
    (
        workspace_path_of(params),
        workspace_identity_of(params),
        params["sessionId"].as_str().unwrap_or_default().to_owned(),
    )
}

fn is_draft(context: &SessionWriteContext, params: &Value) -> bool {
    let (path, identity, session) = draft_key(params);
    context.drafts.has(&path, identity.as_deref(), &session)
}

fn forget_draft(context: &SessionWriteContext, params: &Value) {
    let (path, identity, session) = draft_key(params);
    context.drafts.forget(&path, identity.as_deref(), &session);
}

/// TS `withResolvedMcpServers` 的 `appendWorkspaceToFilesystemMcpServers` 部分。
fn inject_filesystem_workspace(params: &Value, workspace_path: &str) -> Value {
    let Some(servers) = params.get("mcpServers") else {
        return params.clone();
    };
    match append_workspace_to_filesystem_mcp_servers(Some(servers), workspace_path) {
        Some(injected) if &injected != servers => {
            let mut params = params.clone();
            params["mcpServers"] = injected;
            params
        }
        _ => params.clone(),
    }
}

/// 白名单转发（`session_forward_params`）。
async fn forward(
    context: &SessionWriteContext,
    method: &str,
    params: &Value,
) -> Result<Value, Error> {
    let (runtime_method, body) = session_forward_params(method, params).ok_or_else(|| {
        RpcError::new(
            "Fatal",
            format!("no runtime mapping for escode-session.{method}"),
        )
    })?;
    let shaped = Shaped {
        method: runtime_method,
        body,
        with_workspace: false,
        trace: None,
    };
    call(context, params, shaped).await
}

async fn call(
    context: &SessionWriteContext,
    routing: &Value,
    shaped: Shaped,
) -> Result<Value, Error> {
    let RuntimeAccess { instances, options } = &context.runtime;
    let method = shaped.method;
    let answer: RpcResult =
        runtime_call_shaped(instances, options, routing.clone(), shaped, Extract::Result).await;
    match answer? {
        Wire::Object(value) => Ok(value),
        _ => Err(RpcError::new(
            "Fatal",
            format!("runtime `{method}` returned a non-object result"),
        )),
    }
}

/// 把快照写进 tasks-index 并登记任务目标（TS `syncSnapshotAndBroadcast` 的索引部分 +
/// `rememberIndexedTaskMeta`）。失败只记日志（TS 吞错 warn），不影响调用结果。
async fn write_index(
    context: &SessionWriteContext,
    params: &Value,
    snapshot: &Value,
    overrides: IndexOverrides,
    reason: &str,
) {
    let Some(read) = context.read.as_ref() else {
        return;
    };
    let targets = Arc::clone(&read.targets);
    let read = context_ref(read);
    let owned = snapshot.clone();
    let indexed = read
        .run(move |index| {
            sync_snapshot_to_index(index, &owned, &overrides).map_err(|error| error.to_string())
        })
        .await;
    let session_id = snapshot["session"]["sessionId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    match indexed {
        Ok(meta) => {
            remember_from_meta(&targets, &meta);
            // TS `syncSnapshotAndBroadcast`：还没有用户可见内容的快照只写索引不广播（避免 "New session" 闪烁）。
            if has_user_visible_content(snapshot) && meta["taskId"].is_string() {
                context.events.task_list_changed(
                    &crate::workspace_events::Target {
                        workspace_path: meta["workspacePath"].as_str().unwrap_or_default(),
                        workspace_identity: meta["workspaceIdentity"].as_str(),
                        task_id: meta["taskId"].as_str(),
                    },
                    Some(&meta),
                    reason,
                    None,
                );
            }
            // create 的返回体里可能没有 workspace 列：用请求参数兜底登记任务目标。
            let workspace_path = workspace_path_of(params);
            if !session_id.is_empty() && !workspace_path.is_empty() {
                targets.remember(crate::task_targets::TaskTarget {
                    task_id: session_id,
                    workspace_path,
                    workspace_identity: workspace_identity_of(params),
                    cron_automation_id: None,
                    remote_session_id: None,
                });
            }
        }
        Err((code, message)) => {
            eprintln!("escode-host: task index sync failed taskId={session_id}: {code}: {message}");
        }
    }
}
