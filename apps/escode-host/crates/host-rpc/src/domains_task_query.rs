//! `escode-task` 的查询类方法处理函数（`listTaskList` / `listGroupedTaskViewStructure`）。
//! 从 `domains_task.rs` 拆出以守住单文件行数上限；参数整形与 wire 语义见那里的模块注释。

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
