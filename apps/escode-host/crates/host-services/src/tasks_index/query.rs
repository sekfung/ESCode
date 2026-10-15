//! TS `TaskIndexRepo.queryTaskList` / `queryGroupedTaskViewStructure` 的等价实现。
//!
//! `queryTaskList`（服务方法 `listTaskList` 的真相源）：多 workspace 聚合、kind 分区
//! （timeline/active = `pinned=0 AND archived=0`；pinned/archived 各自）、可选 provider 过滤、
//! `LOWER(title) LIKE` 或 `LOWER(searchable_text) LIKE` 命中、`sortBy` 二序、
//! `normalizeLimit`（>0 取整，否则无 LIMIT）、`{items, total, hasMore: total > rows.len}`。
//! 搜索命中行附 `searchSnippet`/`searchSnippets`（窗口半径 20/72、最多 4 条、合并近重叠、
//! 片段 `\s+`→空格后截 140 字符；title 命中但正文未命中回退整段摘要）——逐条对齐
//! `buildSearchSnippets`。
//!
//! `workspacePurpose`（scope 声明的 project/conversation）按 workspace_key 叠加在条目上。

use rusqlite::Connection;
use serde_json::{Value, json};

use super::TasksIndexError;
use super::meta;
use super::read_task_row;
use super::workspace_key;

/// `listTaskList` 的查询参数（TS `ESCodeTaskListQuery` + 注入的 provider）。
pub struct TaskListQuery<'a> {
    pub kind: &'a str,
    /// 参与聚合的 workspace（TS `workspaceScopes`）；空集合回空结果。
    pub workspace_scopes: &'a [WorkspaceScope<'a>],
    pub sort_by: &'a str,
    pub search: Option<&'a str>,
    pub limit: Option<i64>,
    pub provider: Option<&'a str>,
}

/// 一个 workspace scope（`workspacePurpose` 决定条目上是否附 `workspacePurpose`）。
pub struct WorkspaceScope<'a> {
    pub workspace_path: &'a str,
    pub workspace_identity: Option<&'a str>,
    pub workspace_purpose: Option<&'a str>,
}

pub(super) fn query_task_list(
    connection: &Connection,
    query: &TaskListQuery<'_>,
) -> Result<Value, TasksIndexError> {
    let mut workspace_keys: Vec<String> = query
        .workspace_scopes
        .iter()
        .map(|scope| workspace_key(scope.workspace_path, scope.workspace_identity))
        .filter(|key| !key.trim().is_empty())
        .collect();
    workspace_keys.sort();
    workspace_keys.dedup();
    if workspace_keys.is_empty() {
        return Ok(json!({"items": [], "total": 0, "hasMore": false}));
    }

    let search = query
        .search
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let search_like = search.map(|value| format!("%{}%", value.to_lowercase()));

    let mut where_clause = vec!["deleted = 0".to_owned()];
    let placeholders = workspace_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    where_clause.push(format!("workspace_key IN ({placeholders})"));
    let mut args: Vec<rusqlite::types::Value> = workspace_keys
        .iter()
        .map(|key| rusqlite::types::Value::Text(key.clone()))
        .collect();
    if let Some(provider) = query.provider {
        where_clause.push("provider = ?".to_owned());
        args.push(rusqlite::types::Value::Text(provider.to_owned()));
    }
    match query.kind {
        "pinned" => where_clause.push("pinned = 1 AND archived = 0".to_owned()),
        "archived" => where_clause.push("archived = 1".to_owned()),
        // timeline / active 同一分区（TS 的 else 分支）。
        _ => where_clause.push("pinned = 0 AND archived = 0".to_owned()),
    }
    if let Some(like) = &search_like {
        where_clause.push("(LOWER(title) LIKE ? OR LOWER(searchable_text) LIKE ?)".to_owned());
        let like = rusqlite::types::Value::Text(like.clone());
        args.push(like.clone());
        args.push(like);
    }
    let where_clause = where_clause.join(" AND ");

    let total: i64 = connection
        .query_row(
            &format!("SELECT COUNT(1) FROM tasks WHERE {where_clause}"),
            rusqlite::params_from_iter(args.iter()),
            |row| row.get(0),
        )
        .map_err(TasksIndexError::Sqlite)?;

    // `normalizeLimit`：>0 取整，否则无 LIMIT。
    let limit = query.limit.filter(|value| *value > 0);
    let order_by = if query.sort_by == "created" {
        "created_at DESC, updated_at DESC, task_id DESC"
    } else {
        "updated_at DESC, created_at DESC, task_id DESC"
    };
    let sql = match query.limit.filter(|value| *value > 0) {
        Some(_) => format!(
            "SELECT {TASK_ROW_COLUMNS} FROM tasks WHERE {where_clause}
             ORDER BY {order_by} LIMIT ?"
        ),
        None => {
            format!("SELECT {TASK_ROW_COLUMNS} FROM tasks WHERE {where_clause} ORDER BY {order_by}")
        }
    };
    let mut list_args = args.clone();
    if let Some(limit) = limit {
        list_args.push(rusqlite::types::Value::Integer(limit));
    }
    let mut statement = connection.prepare(&sql).map_err(TasksIndexError::Sqlite)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(list_args.iter()), read_task_row)
        .map_err(TasksIndexError::Sqlite)?;

    let purpose_by_key: std::collections::HashMap<&str, &str> = query
        .workspace_scopes
        .iter()
        .filter_map(|scope| {
            scope.workspace_purpose.map(|purpose| {
                let key = workspace_key(scope.workspace_path, scope.workspace_identity);
                (leak_key(&key), purpose)
            })
        })
        .collect();
    let mut items = Vec::new();
    let mut row_count = 0i64;
    for row in rows {
        let row = row.map_err(TasksIndexError::Sqlite)?;
        row_count += 1;
        let mut item = meta::to_task_meta(&row)
            .unwrap_or_else(|| fallback_item(&row))
            .as_object()
            .cloned()
            .unwrap_or_default();
        let snippets = search.and_then(|search| {
            let text = row["searchable_text"].as_str().unwrap_or_default();
            build_search_snippets(text, search)
        });
        if let Some(snippets) = snippets {
            if let Some(first) = snippets.first() {
                item.insert("searchSnippet".into(), json!(first));
            }
            item.insert("searchSnippets".into(), json!(snippets));
        }
        if let Some(purpose) = purpose_by_key.get(row["workspace_key"].as_str().unwrap_or_default())
        {
            item.insert("workspacePurpose".into(), json!(purpose));
        }
        items.push(Value::Object(item));
    }
    Ok(json!({
        "items": items,
        "total": total,
        "hasMore": total > row_count,
    }))
}

use super::TASK_ROW_COLUMNS;
use super::query_shared::{fallback_item, leak_key};
use super::query_snippets::build_search_snippets;

/// `listTaskMetas` 的可选过滤（`None` = 不过滤，与 TS 的 `?? null` 同义）。
#[derive(Default)]
pub struct TaskFilters<'a> {
    pub workspace_path: Option<&'a str>,
    pub workspace_identity: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub include_deleted: bool,
}
