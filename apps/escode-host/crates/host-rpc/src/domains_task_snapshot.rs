//! `escode-task` 的快照读面（S2b）：`getTaskSnapshot` / `getTaskSnapshotWithEtag` /
//! `getTaskSnapshotBody` / `getTaskSnapshotRef` / `getTaskSnapshotToolCallsSlice` /
//! `getTaskConfigOptions`。
//!
//! 顺序与 TS 一致（`escodeTaskServiceAdapter.ts` §2876 起）：
//! 登记任务目标 → （replayable）读索引 hint → `session/resume` → 投影 + 截窗 →
//! 索引回写（返回值的 `meta` 即索引行）→ 可选 ETag 条件返回。
//! `getTaskConfigOptions` 只到 `resume` + 投影 `settings`。
//!
//! 从 `domains_task.rs` 拆出以守住单文件行数上限。

use serde_json::{Value, json};

use escode_host_services::task_projection::{
    config_options_from_snapshot, message_limit_of, not_modified, project_options_of,
    read_session_snapshot, resume_hints, resume_snapshot_request, snapshot_etag,
};

use super::domains_task_support::{
    ReadContext, RuntimeAccess, context_ref, first_object, workspace_identity_of, workspace_path_of,
};
use crate::forward::{Extract, runtime_call};
use crate::session::{ChannelResult, MethodHandler, handler};
use crate::task_targets::{TaskTarget, remember_from_meta};
use crate::wire::Wire;

/// 入参解析结果（TS `TaskTargetWithMcpServers` + 快照选项同形）。
struct SnapshotParams {
    task_id: String,
    workspace_path: String,
    workspace_identity: Option<String>,
    /// 只在 replayable 的两处分支用到：索引 hint 回填与空 elicitation 集合。
    client_mode: Option<String>,
    model: Option<String>,
    thought_level: Option<String>,
    resume_model_policy: Option<String>,
    message_limit: Option<f64>,
    if_none_match: Option<String>,
    mcp_servers: Option<Value>,
    tool_denylist: Option<Value>,
}

impl SnapshotParams {
    fn parse(params: &Value) -> Result<Self, (String, String)> {
        let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
        if task_id.is_empty() {
            return Err((
                "InvalidRequest".to_owned(),
                "taskId is required.".to_owned(),
            ));
        }
        let workspace_path = workspace_path_of(params);
        if workspace_path.is_empty() {
            return Err((
                "InvalidRequest".to_owned(),
                "workspacePath is required.".to_owned(),
            ));
        }
        let text = |key: &str| {
            params[key]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Ok(Self {
            task_id,
            workspace_path,
            workspace_identity: workspace_identity_of(params),
            client_mode: text("clientMode"),
            model: text("model"),
            thought_level: text("thoughtLevel"),
            resume_model_policy: text("resumeModelPolicy"),
            message_limit: message_limit_of(params),
            if_none_match: params["ifNoneMatch"]
                .as_str()
                .map(str::to_owned)
                .filter(|value| !value.is_empty()),
            mcp_servers: params
                .get("mcpServers")
                .filter(|value| !value.is_null())
                .cloned(),
            tool_denylist: params
                .get("toolDenylist")
                .filter(|value| !value.is_null())
                .cloned(),
        })
    }

    fn target(&self) -> TaskTarget {
        TaskTarget {
            task_id: self.task_id.clone(),
            workspace_path: self.workspace_path.clone(),
            workspace_identity: self.workspace_identity.clone(),
            cron_automation_id: None,
            remote_session_id: None,
        }
    }

    /// hint 读取与投影选项共用的参数视图（字段名与 TS 服务面一致）。
    fn to_hints_params(&self) -> Value {
        let mut params = json!({
            "taskId": self.task_id,
            "workspacePath": self.workspace_path,
        });
        let record = params.as_object_mut().expect("object");
        for (key, value) in [
            ("workspaceIdentity", self.workspace_identity.clone()),
            ("clientMode", self.client_mode.clone()),
            ("model", self.model.clone()),
            ("thoughtLevel", self.thought_level.clone()),
            ("resumeModelPolicy", self.resume_model_policy.clone()),
        ] {
            if let Some(value) = value {
                record.insert(key.to_owned(), Value::from(value));
            }
        }
        params
    }

    fn project_options(&self) -> Value {
        let mut params = json!({});
        if let Some(client_mode) = &self.client_mode {
            params["clientMode"] = Value::from(client_mode.clone());
        }
        params
    }
}

/// `getTaskSnapshot`（`with_etag = false`）与 `getTaskSnapshotWithEtag`（`true`）的共用实现。
pub(super) fn snapshot_handler(context: ReadContext, with_etag: bool) -> MethodHandler {
    handler(move |arg: Wire| {
        let params = SnapshotParams::parse(&first_object(arg));
        let context = context_ref(&context);
        Box::pin(async move {
            let params = params?;
            // TS `resumeSnapshot` 第一件事就是 `rememberTaskTarget`（缺省保留既有 cron 归属）。
            context.targets.remember(params.target());
            let snapshot = resume_snapshot(&context, &params).await?;
            let projected = write_projected_snapshot(&context, &params, snapshot).await?;
            if let Some(meta) = projected.get("meta") {
                // 索引 meta 是权威归属：可设置/清空 cronAutomationId（TS `rememberIndexedTaskMeta`）。
                remember_from_meta(&context.targets, meta);
            }
            if !with_etag {
                return Ok(Wire::Object(projected));
            }
            let etag = snapshot_etag(&projected);
            // `ifNoneMatch` 命中：不回快照，只回 etag 与 notModified（客户端据此跳过重渲染）。
            if params.if_none_match.as_deref() == Some(etag.as_str()) {
                return Ok(Wire::Object(not_modified(&etag)));
            }
            Ok(Wire::Object(json!({"snapshot": projected, "etag": etag})))
        })
    })
}

/// `getTaskConfigOptions`：TS 同样走 `resumeSnapshot`（不是 `session/read`），只回 `settings` 投影。
pub(super) fn config_options_handler(context: ReadContext) -> MethodHandler {
    handler(move |arg: Wire| {
        let params = SnapshotParams::parse(&first_object(arg));
        let context = context_ref(&context);
        Box::pin(async move {
            let params = params?;
            context.targets.remember(params.target());
            let snapshot = resume_snapshot(&context, &params).await?;
            let Wire::Object(snapshot) = snapshot else {
                return Err((
                    "Fatal".to_owned(),
                    "session/resume returned a non-object result".to_owned(),
                ));
            };
            Ok(Wire::Object(config_options_from_snapshot(&snapshot)))
        })
    })
}

/// TS `getTaskSnapshotBody` / `getTaskSnapshotRef` / `getTaskSnapshotToolCallsSlice`：
/// 适配器本身 `return null`（该 Host 不实现），Rust 侧照抄该语义（不启动实例、不读索引）。
pub(super) fn null_handler() -> MethodHandler {
    handler(|_arg: Wire| Box::pin(async move { Ok(Wire::Object(Value::Null)) }))
}

/// TS `updateIndexedTaskState` 的兜底：旧会话可能还没有索引行——按需 resume 并把快照写回索引
/// （`syncTaskIndexSnapshot(await resumeSnapshot(params))`），之后调用方重试状态写。
pub(super) async fn seed_from_runtime(
    context: &ReadContext,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
) -> Result<(), (String, String)> {
    let mut raw = json!({"taskId": task_id, "workspacePath": workspace_path});
    if let Some(identity) = workspace_identity {
        raw["workspaceIdentity"] = Value::from(identity);
    }
    let params = SnapshotParams::parse(&raw)?;
    let snapshot = resume_snapshot(context, &params).await?;
    write_projected_snapshot(context, &params, snapshot)
        .await
        .map(|_| ())
}

/// `resumeSnapshot`：hint 读取（只有 replayable）→ `session/resume` 转发。
async fn resume_snapshot(context: &ReadContext, params: &SnapshotParams) -> ChannelResult {
    let Some(runtime) = context.runtime.clone() else {
        return Err((
            "NotImplemented".to_owned(),
            "escode-task runtime access is not configured for this Host".to_owned(),
        ));
    };
    let thought_level = resolve_thought_level_hint(context, params).await;
    let request = resume_snapshot_request(
        &params.task_id,
        &params.workspace_path,
        params.workspace_identity.as_deref(),
        thought_level.as_deref(),
        params.mcp_servers.as_ref(),
        params.tool_denylist.as_ref(),
    );
    call_runtime(&runtime, request).await
}

/// TS `resolveTaskIndexResumeHints`：只有手机 replayable 首屏回填；读失败按「无 hint」继续。
async fn resolve_thought_level_hint(
    context: &ReadContext,
    params: &SnapshotParams,
) -> Option<String> {
    let hints_params = params.to_hints_params();
    let workspace_path = params.workspace_path.clone();
    let identity = params.workspace_identity.clone();
    context
        .run(move |index| {
            resume_hints(
                Some(index),
                &hints_params,
                &workspace_path,
                identity.as_deref(),
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map(|(_, thought_level)| thought_level)
        .unwrap_or(None)
}

async fn call_runtime(runtime: &RuntimeAccess, request: Value) -> ChannelResult {
    runtime_call(
        &runtime.instances,
        &runtime.options,
        "session/resume",
        request,
        Extract::Result,
    )
    .await
    .map_err(Into::into)
}

/// 投影 + 截窗 + 索引回写（同步 SQLite IO 在 `ReadContext::run` 的专用线程里执行）。
async fn write_projected_snapshot(
    context: &ReadContext,
    params: &SnapshotParams,
    snapshot: Wire,
) -> Result<Value, (String, String)> {
    let Wire::Object(snapshot) = snapshot else {
        return Err((
            "Fatal".to_owned(),
            "session/resume returned a non-object result".to_owned(),
        ));
    };
    let message_limit = params.message_limit;
    let options = project_options_of(&params.project_options());
    context
        .run(move |index| {
            read_session_snapshot(index, &snapshot, message_limit, options)
                .map_err(|error| error.to_string())
        })
        .await
}
