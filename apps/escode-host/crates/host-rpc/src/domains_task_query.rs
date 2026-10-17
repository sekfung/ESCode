//! `escode-task` 的查询类方法处理函数（`listTaskList` / `listGroupedTaskViewStructure` /
//! `getTaskModelSelection`）。从 `domains_task.rs` 拆出以守住单文件行数上限；
//! 参数整形与 wire 语义见那里的模块注释。

use escode_host_services::tasks_index::query::{TaskListQuery, WorkspaceScope};
use serde_json::Value;

use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

use crate::domains_task_support::{GLM_PROVIDER, ReadContext, context_ref, first_object};

/// `listTaskList`：多 workspace 聚合 + kind 分区 + 搜索/分页。
pub(super) fn list_task_list_handler(context: ReadContext) -> MethodHandler {
    handler(move |arg: Wire| {
        let params = first_object(arg);
        let context = context_ref(&context);
        Box::pin(async move {
            let empty_scopes: Vec<Value> = Vec::new();
            let scopes: Vec<(String, Option<String>, Option<String>)> = params["workspaceScopes"]
                .as_array()
                .unwrap_or(&empty_scopes)
                .iter()
                .map(|scope| {
                    (
                        scope["workspacePath"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        scope["workspaceIdentity"]
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned),
                        scope["workspacePurpose"].as_str().map(str::to_owned),
                    )
                })
                .collect();
            let kind = params["kind"].as_str().unwrap_or("active").to_owned();
            let sort_by = params["sortBy"].as_str().unwrap_or("updated").to_owned();
            let search = params["search"].as_str().map(str::to_owned);
            let limit = params["limit"].as_i64();
            context
                .run(move |index| {
                    let scopes: Vec<WorkspaceScope<'_>> = scopes
                        .iter()
                        .map(|(path, identity, purpose)| WorkspaceScope {
                            workspace_path: path.as_str(),
                            workspace_identity: identity.as_deref(),
                            workspace_purpose: purpose.as_deref(),
                        })
                        .collect();
                    let query = TaskListQuery {
                        kind: &kind,
                        workspace_scopes: &scopes,
                        sort_by: &sort_by,
                        search: search.as_deref(),
                        limit,
                        provider: Some(GLM_PROVIDER),
                    };
                    index
                        .query_task_list(&query)
                        .map(Wire::Object)
                        .map_err(|error| error.to_string())
                })
                .await
        })
    })
}

/// `getTaskModelSelection`：只有 `{taskId}` 入参 —— 注册表解析目标后读运行时会话的当前模型。
pub(super) fn get_task_model_selection_handler(context: ReadContext) -> MethodHandler {
    handler(move |arg: Wire| {
        let params = first_object(arg);
        let context = context_ref(&context);
        Box::pin(async move {
            let task_id = params["taskId"].as_str().unwrap_or_default().to_owned();
            if task_id.is_empty() {
                return Err((
                    "InvalidRequest".to_owned(),
                    "taskId is required.".to_owned(),
                ));
            }
            let target = context.targets.get(&task_id)?;
            let Some(runtime) = context.runtime.as_ref() else {
                return Err((
                    "NotImplemented".to_owned(),
                    "escode-task runtime access is not configured for this Host".to_owned(),
                ));
            };
            let mut request = serde_json::json!({
                "workspacePath": target.workspace_path,
                "sessionId": task_id,
            });
            if let Some(identity) = target.workspace_identity.as_ref() {
                request["workspaceIdentity"] = Value::String(identity.clone());
            }
            // TS `getTaskModelSelection`：直接读运行时 `session/read` 的 `settings.model.current`。
            let answer = crate::forward::runtime_call(
                &runtime.instances,
                &runtime.options,
                "session/read",
                request,
                crate::forward::Extract::Result,
            )
            .await?;
            // `Extract::Result` 已把 `result` 解包成对象；服务面只回 `settings.model.current`。
            let Wire::Object(answer) = answer else {
                return Err((
                    "Fatal".to_owned(),
                    "session/read returned a non-object result".to_owned(),
                ));
            };
            let current = answer
                .get("settings")
                .and_then(|settings| settings.get("model"))
                .and_then(|model| model.get("current"))
                .cloned()
                .unwrap_or(Value::Null);
            Ok(if current.is_null() {
                Wire::Undefined
            } else {
                Wire::Object(current)
            })
        })
    })
}

/// `listGroupedTaskViewStructure`：group/成员/顶层排序原始结构（不 join tasks 表）。
pub(super) fn list_grouped_structure_handler(context: ReadContext) -> MethodHandler {
    handler(move |arg: Wire| {
        let params = first_object(arg);
        let context = context_ref(&context);
        Box::pin(async move {
            let empty_scopes: Vec<Value> = Vec::new();
            let scopes: Vec<(String, Option<String>)> = params["workspaceScopes"]
                .as_array()
                .unwrap_or(&empty_scopes)
                .iter()
                .map(|scope| {
                    (
                        scope["workspacePath"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        scope["workspaceIdentity"]
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned),
                    )
                })
                .collect();
            context
                .run(move |index| {
                    let borrowed: Vec<(&str, Option<&str>)> = scopes
                        .iter()
                        .map(|(path, identity)| (path.as_str(), identity.as_deref()))
                        .collect();
                    index
                        .query_grouped_task_view_structure(&borrowed)
                        .map(Wire::Object)
                        .map_err(|error| error.to_string())
                })
                .await
        })
    })
}
