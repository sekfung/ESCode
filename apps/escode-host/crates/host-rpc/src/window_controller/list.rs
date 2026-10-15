//! `window-controller.listTaskList`（TS `windowHostControllerService.ts:472-584` 的本地 source 分支）。
//!
//! 搜索分支：走索引 DB 全文搜索（含 snippets），再与投影行逐条叠加；
//! 非搜索分支：只取投影 rows（membership 过滤），不读库。
//! 排序/分页口径见 docs/specs/rust-u2-window-controller.md §2。

use std::collections::BTreeSet;
use std::sync::Arc;

use escode_host_services::tasks_index::query::{TaskListQuery, WorkspaceScope};
use serde_json::{Value, json};

use super::WindowController;
use super::overlays;
use super::rows::{LocalScope, matches_membership_kind};
use crate::collation;
use crate::domains_task_support::{GLM_PROVIDER, first_object};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

pub(super) fn list_handler(controller: &Arc<WindowController>) -> MethodHandler {
    let controller = Arc::clone(controller);
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let controller = Arc::clone(&controller);
        Box::pin(async move { list_task_list(&controller, params).await.map(Wire::Object) })
    })
}

/// TS `listTaskList`：解析 source → 逐个刷新（失败只记录）→ 搜索/投影分支 → 排序分页。
async fn list_task_list(
    controller: &Arc<WindowController>,
    query: Value,
) -> Result<Value, RpcError> {
    let scopes = resolve_scopes(&query);
    for scope in &scopes {
        if let Err(error) = controller.refresh(scope, false).await {
            // 一个 source 的瞬时读取失败不能清空可信投影（TS `onSourceError` 只记录）。
            eprintln!(
                "escode-host: window-controller refresh failed for {}: {}",
                scope.workspace_path, error.message
            );
        }
    }

    let kind = query["kind"].as_str().unwrap_or("active").to_owned();
    let sort_by = query["sortBy"].as_str().unwrap_or("updated").to_owned();
    let search = query["search"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let limit = query["limit"].as_u64();

    let mut items = match &search {
        Some(search) => search_items(controller, &query, &scopes, search).await,
        None => projection_items(controller, &scopes, &kind),
    };
    items.sort_by(|left, right| compare_items(left, right, &sort_by));
    let total = items.len();
    let visible = match limit {
        Some(limit) => {
            let mut visible = items;
            visible.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
            visible
        }
        None => items,
    };
    Ok(json!({
        "items": visible,
        "total": total,
        "hasMore": total > visible.len(),
    }))
}

/// TS `resolveQuerySources` 的本地分支：远程 identity 的 scope 不回落本地（登记差异 §5-1），
/// 按 source key 去重。
fn resolve_scopes(query: &Value) -> Vec<LocalScope> {
    let empty: Vec<Value> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut scopes = Vec::new();
    for scope in query["workspaceScopes"].as_array().unwrap_or(&empty) {
        let path = scope["workspacePath"].as_str().unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        let identity = scope["workspaceIdentity"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if identity.is_some_and(super::is_remote_workspace_identity) {
            continue;
        }
        let local = LocalScope::new(path, identity);
        if seen.insert(local.source_key()) {
            scopes.push(local);
        }
    }
    scopes
}

/// 搜索分支：每个在线 source 各查一次（单 source 失败 → 记日志 + 空结果，不影响其它 source）。
async fn search_items(
    controller: &Arc<WindowController>,
    query: &Value,
    scopes: &[LocalScope],
    search: &str,
) -> Vec<Value> {
    let mut items = Vec::new();
    for scope in scopes {
        match search_source(controller, query, scope, search).await {
            Ok(mut found) => items.append(&mut found),
            Err(error) => eprintln!(
                "escode-host: window-controller search failed for {}: {}",
                scope.workspace_path, error.message
            ),
        }
    }
    items
}

async fn search_source(
    controller: &Arc<WindowController>,
    query: &Value,
    scope: &LocalScope,
    search: &str,
) -> Result<Vec<Value>, RpcError> {
    // TS：`{...query, workspaceScopes:[source], limit: undefined}`（本 source 全量交给投影层截断）。
    let kind = query["kind"].as_str().unwrap_or("active").to_owned();
    let sort_by = query["sortBy"].as_str().unwrap_or("updated").to_owned();
    let path = scope.workspace_path.clone();
    let identity = scope.workspace_identity.clone();
    let search = search.to_owned();
    let result = controller
        .read()
        .run(move |index| {
            let scopes = [WorkspaceScope {
                workspace_path: &path,
                workspace_identity: identity.as_deref(),
                workspace_purpose: None,
            }];
            let query = TaskListQuery {
                kind: &kind,
                workspace_scopes: &scopes,
                sort_by: &sort_by,
                search: Some(&search),
                limit: None,
                provider: Some(GLM_PROVIDER),
            };
            index
                .query_task_list(&query)
                .map_err(|error| error.to_string())
        })
        .await?;
    let Some(items) = result["items"].as_array() else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .map(|item| decorate_search_item(controller, scope, item))
        .collect())
}

/// TS 搜索分支的逐条叠加：`{...(projected?.meta ?? normalized), sourceAvailability, liveStatus, activity?, searchSnippets?}`。
fn decorate_search_item(controller: &WindowController, scope: &LocalScope, item: &Value) -> Value {
    let task_id = item["taskId"].as_str().unwrap_or_default().to_owned();
    let normalized = {
        let mut meta = item.clone();
        super::normalize_scope(&mut meta, scope);
        meta
    };
    let projected = controller
        .projector()
        .row(&scope.address(&task_id), &task_id);
    let mut decorated = projected
        .as_ref()
        .map(|row| row["meta"].clone())
        .unwrap_or_else(|| normalized.clone());
    let live_status = projected
        .as_ref()
        .and_then(|row| row["liveStatus"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| overlays::default_live_status(&normalized).to_owned());
    if let Some(record) = decorated.as_object_mut() {
        record.insert("sourceAvailability".to_owned(), Value::from("online"));
        record.insert("liveStatus".to_owned(), Value::from(live_status));
        if let Some(activity) = projected.as_ref().and_then(|row| row.get("activity")) {
            record.insert("activity".to_owned(), activity.clone());
        }
        if let Some(snippets) = item.get("searchSnippets").filter(|value| !value.is_null()) {
            record.insert("searchSnippets".to_owned(), snippets.clone());
        }
    }
    decorated
}

/// TS 非搜索分支：投影 rows → membership 过滤 → 叠加 `sourceAvailability/liveStatus/activity?`。
fn projection_items(
    controller: &WindowController,
    scopes: &[LocalScope],
    kind: &str,
) -> Vec<Value> {
    let keys: Vec<String> = scopes.iter().map(LocalScope::source_key).collect();
    controller
        .projector()
        .rows(&keys)
        .into_iter()
        .filter(|row| matches_membership_kind(&row["membership"], kind))
        .map(|row| {
            let mut item = row["meta"].clone();
            if let Some(record) = item.as_object_mut() {
                record.insert(
                    "sourceAvailability".to_owned(),
                    row["sourceAvailability"].clone(),
                );
                record.insert("liveStatus".to_owned(), row["liveStatus"].clone());
                if let Some(activity) = row.get("activity") {
                    record.insert("activity".to_owned(), activity.clone());
                }
                if let Some(snippets) = row.get("searchSnippets").filter(|value| !value.is_null()) {
                    record.insert("searchSnippets".to_owned(), snippets.clone());
                }
            }
            item
        })
        .collect()
}

/// TS `compareItems`：`sortBy === "created"` 取 createdAt，其余取 updatedAt，降序 → taskId `localeCompare`。
fn compare_items(left: &Value, right: &Value, sort_by: &str) -> std::cmp::Ordering {
    let key = if sort_by == "created" {
        "createdAt"
    } else {
        "updatedAt"
    };
    let left_at = left[key].as_i64().unwrap_or(0);
    let right_at = right[key].as_i64().unwrap_or(0);
    right_at.cmp(&left_at).then_with(|| {
        collation::compare_names(
            left["taskId"].as_str().unwrap_or_default(),
            right["taskId"].as_str().unwrap_or_default(),
        )
    })
}
