//! `escode-task.createTask`（TS `escodeTaskServiceAdapter.ts:1911-2055`）。
//!
//! 本片覆盖 UI 主路径：新建 session、复用 draft（必要时同步模型/思考强度）、自动化/闲时标记
//! （`persistence: deferred` + 索引归属字段）、`task_created` 事件与 `initialSlashCommands`。
//! 登记差异：`v4Create`（Bot/host 路径，尚未迁移）显式拒绝；新建任务的 grouped-view 置顶
//! 与 syncer 显式通知未实现（V4 sessions-index 帧到达后自然收敛）。

use serde_json::{Value, json};

use escode_host_services::session_params::{
    ToolGates, append_workspace_to_filesystem_mcp_servers, session_create_params,
    session_forward_params, with_api_retry_runtime,
};
use escode_host_services::task_projection::{IndexOverrides, sync_snapshot_to_index};
use escode_host_services::tasks_index::SyncOptions;

use crate::domains_task_state::{StateContext, method};
use crate::domains_task_support::context_ref;
use crate::forward::{self, Extract, Shaped};
use crate::session::{MethodHandler, RpcError};
use crate::wire::Wire;
use crate::workspace_events::Target;

pub(super) fn create_task(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let workspace_path = params["workspacePath"]
            .as_str()
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| RpcError::new("InvalidRequest", "workspacePath is required."))?;
        let identity = params["workspaceIdentity"].as_str().map(str::to_owned);
        if params["v4Create"].as_bool() == Some(true) {
            return Err(RpcError::new(
                "Error",
                "escode-task.createTask v4Create is not supported by the Rust Host yet",
            )
            .with("code", Value::from("ESCODE_TASK_V4_CREATE_UNSUPPORTED")));
        }
        let Some(runtime) = context.read.runtime.clone() else {
            return Err(RpcError::new(
                "InvalidRequest",
                "Host has no runtime access.",
            ));
        };
        let selection = model_selection(&params);
        let thought_level = selection
            .as_ref()
            .and_then(|selection| selection["options"]["reasoningLevel"].as_str())
            .or_else(|| params["thoughtLevel"].as_str())
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(str::to_owned);
        let automation = params["automationId"].as_str().map(str::to_owned);
        let offpeak = params["offPeakTaskId"].as_str().map(str::to_owned);
        let defer = params["deferPersistenceUntilFirstPrompt"].as_bool() == Some(true)
            || automation.is_some();
        let mcp_servers =
            append_workspace_to_filesystem_mcp_servers(params.get("mcpServers"), &workspace_path);

        // draft 复用：只读取到 snapshot 时同步模型/思考强度；会话已失效则降级新建。
        let draft_id = params["draftSessionId"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let mut snapshot = None;
        if let Some(draft_id) = draft_id
            && mcp_servers.is_none()
        {
            match read_snapshot(&runtime, &params, &draft_id).await {
                Ok(found) => {
                    let mut found = found;
                    let current = &found["settings"]["model"]["current"];
                    if let Some(selection) = &selection
                        && !same_model_selection(current, selection)
                    {
                        found = call_session(
                            &runtime,
                            &params,
                            "session/setModel",
                            json!({"sessionId": draft_id, "model": selection}),
                            false,
                        )
                        .await?;
                    }
                    if let Some(level) = &thought_level
                        && found["settings"]["thoughtLevel"]["current"].as_str() != Some(level)
                    {
                        found = call_session(
                            &runtime,
                            &params,
                            "session/setThoughtLevel",
                            json!({"sessionId": draft_id, "thoughtLevel": level}),
                            false,
                        )
                        .await?;
                    }
                    snapshot = Some(found);
                }
                Err(error) if is_session_missing(&error) => {}
                Err(error) => return Err(error),
            }
        }

        let snapshot = match snapshot {
            Some(snapshot) => snapshot,
            None => {
                let trace_id = uuid::Uuid::new_v4().to_string();
                let mut create = serde_json::Map::new();
                create.insert("sessionTraceId".to_owned(), Value::from(trace_id.clone()));
                if let Some(mode) = to_session_mode(params["mode"].as_str()) {
                    create.insert("mode".to_owned(), Value::from(mode));
                }
                if let Some(selection) = &selection {
                    create.insert("model".to_owned(), selection.clone());
                }
                if let Some(level) = &thought_level {
                    create.insert("thoughtLevel".to_owned(), Value::from(level.clone()));
                }
                if defer {
                    create.insert("persistence".to_owned(), Value::from("deferred"));
                }
                if automation.is_some() {
                    create.insert("titleGenerationEnabled".to_owned(), Value::Bool(false));
                }
                if let Some(servers) = &mcp_servers {
                    create.insert("mcpServers".to_owned(), servers.clone());
                }
                let shaped = Shaped {
                    method: "session/create",
                    body: session_create_params(&Value::Object(create), &ToolGates::default()),
                    with_workspace: true,
                    trace: Some(json!({"traceId": trace_id})),
                };
                with_api_retry_runtime(runtime_object(&runtime, &params, shaped).await?)
            }
        };

        let (session_id, meta) = index_snapshot(&context, &snapshot).await?;
        let meta = match (automation.clone(), offpeak.clone()) {
            (None, None) => meta,
            (automation, offpeak) => {
                let mut merged = meta.as_object().cloned().unwrap_or_default();
                if let Some(automation) = automation {
                    merged.insert("cronAutomationId".to_owned(), Value::from(automation));
                }
                if let Some(offpeak) = offpeak {
                    merged.insert("offPeakTaskId".to_owned(), Value::from(offpeak));
                }
                let object = merged.clone();
                context_ref(&context.read)
                    .run(move |index| {
                        index
                            .sync_task_meta(&object, &SyncOptions::default())
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(RpcError::from)?
            }
        };
        context.read.targets.remember_indexed_meta(
            &session_id,
            &workspace_path,
            identity.as_deref(),
            meta["cronAutomationId"].as_str(),
        );
        context.events.task_list_changed(
            &Target {
                workspace_path: &workspace_path,
                workspace_identity: identity.as_deref(),
                task_id: Some(&session_id),
            },
            Some(&meta),
            "task_created",
            None,
        );
        let mut result = meta;
        result["initialSlashCommands"] = snapshot
            .get("slashCommands")
            .cloned()
            .unwrap_or_else(|| json!([]));
        Ok(Wire::Object(result))
    })
}

async fn index_snapshot(
    context: &StateContext,
    snapshot: &Value,
) -> Result<(String, Value), RpcError> {
    let owned = snapshot.clone();
    let meta = context_ref(&context.read)
        .run(move |index| {
            sync_snapshot_to_index(index, &owned, &IndexOverrides::default())
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(RpcError::from)?;
    let session_id = meta["taskId"].as_str().unwrap_or_default().to_owned();
    if session_id.is_empty() {
        return Err(RpcError::new("Fatal", "session snapshot has no sessionId"));
    }
    Ok((session_id, meta))
}

pub(super) async fn read_snapshot(
    runtime: &crate::domains_task_support::RuntimeAccess,
    routing: &Value,
    draft_id: &str,
) -> Result<Value, RpcError> {
    let params = json!({"sessionId": draft_id});
    let (method, body) = session_forward_params("readSession", &params)
        .ok_or_else(|| RpcError::new("Fatal", "no runtime mapping for readSession"))?;
    runtime_object(
        runtime,
        routing,
        Shaped {
            method,
            body,
            with_workspace: false,
            trace: None,
        },
    )
    .await
}

pub(super) async fn call_session(
    runtime: &crate::domains_task_support::RuntimeAccess,
    routing: &Value,
    method: &str,
    params: Value,
    with_workspace: bool,
) -> Result<Value, RpcError> {
    let (runtime_method, body) = session_forward_params(method, &params)
        .ok_or_else(|| RpcError::new("Fatal", format!("no runtime mapping for {method}")))?;
    let snapshot = runtime_object(
        runtime,
        routing,
        Shaped {
            method: runtime_method,
            body,
            with_workspace,
            trace: None,
        },
    )
    .await?;
    Ok(with_api_retry_runtime(snapshot))
}

async fn runtime_object(
    runtime: &crate::domains_task_support::RuntimeAccess,
    routing: &Value,
    shaped: Shaped,
) -> Result<Value, RpcError> {
    let method = shaped.method;
    let answer = forward::runtime_call_shaped(
        &runtime.instances,
        &runtime.options,
        routing.clone(),
        shaped,
        Extract::Result,
    )
    .await?;
    match answer {
        Wire::Object(value) => Ok(value),
        _ => Err(RpcError::new(
            "Fatal",
            format!("runtime `{method}` returned a non-object result"),
        )),
    }
}

fn model_selection(params: &Value) -> Option<Value> {
    if let Some(selection) = params
        .get("modelSelection")
        .filter(|value| value.is_object())
    {
        return Some(selection.clone());
    }
    let raw = params["model"].as_str()?.trim();
    parse_model_picker(raw)
}

/// TS `parseModelPickerValue` 的主路径：`provider/model[$reasoning]`。
pub(super) fn parse_model_picker(value: &str) -> Option<Value> {
    let (provider, rest) = value.split_once('/')?;
    if provider.is_empty() || rest.is_empty() {
        return None;
    }
    let (model, level) = match rest.rsplit_once('$') {
        Some((model, level)) if !model.is_empty() && !level.is_empty() => (model, Some(level)),
        _ => (rest, None),
    };
    let mut selection = json!({"providerId": provider, "modelId": model});
    if let Some(level) = level {
        selection["options"] = json!({"reasoningLevel": level});
    }
    Some(selection)
}

fn same_model_selection(left: &Value, right: &Value) -> bool {
    left["providerId"] == right["providerId"]
        && left["modelId"] == right["modelId"]
        && left["options"]["reasoningLevel"] == right["options"]["reasoningLevel"]
}

fn is_session_missing(error: &RpcError) -> bool {
    let message = error.message.to_lowercase();
    message.contains("session not found") || message.contains("session is not active")
}

pub(super) fn to_session_mode(mode: Option<&str>) -> Option<&'static str> {
    Some(match mode {
        Some("plan") => "plan",
        Some("edit") => "edit",
        Some("guarded") => "guarded",
        Some("yolo" | "bypassPermissions" | "dontAsk") => "yolo",
        Some("auto") => "auto",
        Some("build" | "default" | "acceptEdits" | "autoEdit") => "build",
        _ => return None,
    })
}
