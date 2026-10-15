//! tasks-index 的**状态写**（TS `TaskIndexRepo.updateTaskState` / `clearTaskUnreadIfMatches` /
//! `deleteArchivedTask`，`packages/services/src/session/taskIndexRepo.ts:1852-1997`）。
//!
//! 不能简化的规则：
//! - unread 标记在 SQLite 写锁内按持久 watermark 分配**严格递增**值（毫秒时间戳会碰撞，清除后只看
//!   当前值会复用旧版本）：`max(请求值, max(last_unread_at, unread_at, meta.unreadAt) + 1)`；
//! - 删除标记与分组引用清理同一事务提交（否则 sessions-index、可见性与分组归属长期矛盾）；
//! - 清未读的 CAS：比较与写入持有同一写事务（手机旧点击不能清掉随后产生的新未读）；
//! - 删归档：归档检查与 tombstone 写入同事务（确认框停留期间被另一端恢复时跳过）。

use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::write::{WriteRecord, write_record};
use super::{TasksIndexError, meta, workspace_key};

/// 「在 patch 里出现」的三态（TS 用 `"key" in patch` 区分「清空」与「不改」）。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Field<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

/// TS `TaskIndexStatePatch`。
#[derive(Clone, Debug, Default)]
pub struct StatePatch {
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub deleted: Option<bool>,
    pub title: Option<String>,
    pub title_overridden: Option<bool>,
    pub model: Option<String>,
    pub updated_at: Option<i64>,
    pub unread_at: Field<i64>,
    pub status: Option<String>,
    pub last_error: Field<Value>,
    pub target: Field<Value>,
}

/// 一个任务的定位（身份键 + taskId）。
pub struct TaskRef<'a> {
    pub workspace_path: &'a str,
    pub workspace_identity: Option<&'a str>,
    pub task_id: &'a str,
}

/// 行状态：meta 投影 + 布尔标记 + unread watermark。
struct RowState {
    meta: Map<String, Value>,
    workspace_key: String,
    task_id: String,
    pinned: bool,
    archived: bool,
    deleted: bool,
    title_overridden: bool,
    unread_at: Option<i64>,
    last_unread_at: i64,
}

fn read_state(
    connection: &Connection,
    task: &TaskRef<'_>,
) -> Result<Option<RowState>, TasksIndexError> {
    let key = workspace_key(task.workspace_path, task.workspace_identity);
    let mut statement = connection
        .prepare(&format!(
            "SELECT {columns}, pinned, archived, deleted, title_overridden, unread_at, last_unread_at
             FROM tasks WHERE workspace_key = ?1 AND task_id = ?2",
            columns = super::TASK_ROW_COLUMNS
        ))
        .map_err(TasksIndexError::Sqlite)?;
    let base = super::TASK_ROW_COLUMN_COUNT;
    let found = statement.query_row(params![key, task.task_id], |row| {
        Ok((
            super::read_task_row(row)?,
            row.get::<_, i64>(base)? == 1,
            row.get::<_, i64>(base + 1)? == 1,
            row.get::<_, i64>(base + 2)? == 1,
            row.get::<_, i64>(base + 3)? == 1,
            row.get::<_, Option<i64>>(base + 4)?,
            row.get::<_, Option<i64>>(base + 5)?.unwrap_or(0),
        ))
    });
    let (row, pinned, archived, deleted, title_overridden, unread_at, last_unread_at) = match found
    {
        Ok(found) => found,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(TasksIndexError::Sqlite(error)),
    };
    // TS `rowToMeta`：合法 meta_json 投影，否则列回退。
    let meta = meta::to_task_meta(&row)
        .unwrap_or_else(|| meta::fallback_meta(&row))
        .as_object()
        .cloned()
        .unwrap_or_default();
    Ok(Some(RowState {
        meta,
        workspace_key: row["workspace_key"].as_str().unwrap_or_default().to_owned(),
        task_id: row["task_id"].as_str().unwrap_or_default().to_owned(),
        pinned,
        archived,
        deleted,
        title_overridden,
        unread_at,
        last_unread_at,
    }))
}

/// 在事务里执行（失败回滚）。
fn transaction<T>(
    connection: &Connection,
    body: impl FnOnce() -> Result<T, TasksIndexError>,
) -> Result<T, TasksIndexError> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(TasksIndexError::Sqlite)?;
    match body() {
        Ok(value) => {
            connection
                .execute_batch("COMMIT")
                .map_err(TasksIndexError::Sqlite)?;
            Ok(value)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// TS `deleteTaskGroupingReferencesReady`（第二个 `node_key = ?` 绑定 workspaceKey 与 TS 逐字一致）。
fn delete_grouping_references(
    connection: &Connection,
    workspace_key: &str,
    task_id: &str,
) -> Result<(), TasksIndexError> {
    connection
        .execute(
            "DELETE FROM task_group_members WHERE workspace_key = ?1 AND task_id = ?2",
            params![workspace_key, task_id],
        )
        .map_err(TasksIndexError::Sqlite)?;
    let node_key = serde_json::to_string(&[workspace_key, task_id]).unwrap_or_default();
    connection
        .execute(
            "DELETE FROM task_group_view_node_orders
             WHERE node_type = 'task' AND (node_key = ?1 OR node_key = ?2)",
            params![node_key, workspace_key],
        )
        .map_err(TasksIndexError::Sqlite)?;
    Ok(())
}

fn not_found(task_id: &str) -> TasksIndexError {
    TasksIndexError::TaskNotFound {
        task_id: task_id.to_owned(),
    }
}

/// TS `updateTaskState`。
pub fn update_task_state(
    connection: &Connection,
    task: &TaskRef<'_>,
    patch: &StatePatch,
) -> Result<Value, TasksIndexError> {
    let deleting = patch.deleted == Some(true);
    let mutating_unread = patch.unread_at != Field::Keep;
    let write = || -> Result<Value, TasksIndexError> {
        let state = read_state(connection, task)?
            .filter(|state| !state.deleted)
            .ok_or_else(|| not_found(task.task_id))?;
        let current_unread = state.meta.get("unreadAt").and_then(Value::as_i64);
        let watermark = state
            .last_unread_at
            .max(state.unread_at.unwrap_or(0))
            .max(current_unread.unwrap_or(0));
        let unread = match patch.unread_at {
            Field::Set(requested) => Some(requested.max(watermark + 1)),
            Field::Clear => None,
            Field::Keep => current_unread,
        };
        let mut next = state.meta.clone();
        if let Some(title) = &patch.title {
            next.insert("title".to_owned(), Value::from(title.clone()));
        }
        if let Some(overridden) = patch.title_overridden {
            next.insert("titleOverridden".to_owned(), Value::Bool(overridden));
        }
        if let Some(model) = &patch.model {
            next.insert("model".to_owned(), Value::from(model.clone()));
        }
        if let Some(updated_at) = patch.updated_at {
            next.insert("updatedAt".to_owned(), Value::from(updated_at));
        }
        match unread {
            Some(value) => next.insert("unreadAt".to_owned(), Value::from(value)),
            None => next.remove("unreadAt"),
        };
        if let Some(status) = &patch.status {
            next.insert("status".to_owned(), Value::from(status.clone()));
        }
        apply_field(&mut next, "lastError", &patch.last_error);
        apply_field(&mut next, "target", &patch.target);
        let persisted = write_record(
            connection,
            WriteRecord {
                meta: next,
                pinned: patch.pinned.unwrap_or(state.pinned),
                archived: patch.archived.unwrap_or(state.archived),
                deleted: patch.deleted.unwrap_or(state.deleted),
                title_overridden: patch.title_overridden.unwrap_or(state.title_overridden),
                write_unread_at: mutating_unread,
                searchable_text: None,
            },
        )?;
        if deleting {
            delete_grouping_references(connection, &state.workspace_key, &state.task_id)?;
        }
        Ok(persisted)
    };
    if deleting || mutating_unread {
        transaction(connection, write)
    } else {
        write()
    }
}

fn apply_field(meta: &mut Map<String, Value>, key: &str, field: &Field<Value>) {
    match field {
        Field::Keep => {}
        Field::Set(value) => {
            meta.insert(key.to_owned(), value.clone());
        }
        Field::Clear => {
            meta.remove(key);
        }
    }
}

/// TS `applyAgentPatch` 的入参（`title`/`status` 缺席 = 不改；`lastError`/`target` 三态）。
#[derive(Clone, Debug, Default)]
pub struct AgentPatch {
    pub title: Option<String>,
    pub status: Option<String>,
    pub updated_at: Option<i64>,
    pub last_error: Field<Value>,
    pub target: Field<Value>,
}

/// TS `TaskIndexRepo.applyAgentPatch`：行缺失/已删回 `None`（调用方据此回源完整快照），
/// 不抛错——agent 后台补丁对不存在的行没有用户可见失败面。
///
/// 不能简化的规则：`title_overridden` 时拒收 agent 标题（用户手动标题不被后台刷新冲掉，
/// `write.rs` 合并语义在补丁路径的对应物）；行 flag 全量保留；`searchable_text` 不动。
pub fn apply_agent_patch(
    connection: &Connection,
    task: &TaskRef<'_>,
    patch: &AgentPatch,
) -> Result<Option<Value>, TasksIndexError> {
    let Some(state) = read_state(connection, task)?.filter(|state| !state.deleted) else {
        return Ok(None);
    };
    let mut next = state.meta;
    // TS `canAcceptAgentTitle && params.patch.title`：JS 真值判断，空串才算缺省（不 trim，
    // 调用方 processSummary 已传 trim 后的标题）。
    if let Some(title) = patch
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .filter(|_| !state.title_overridden)
    {
        next.insert("title".to_owned(), Value::from(title.to_owned()));
    }
    next.insert(
        "titleOverridden".to_owned(),
        Value::Bool(state.title_overridden),
    );
    if let Some(updated_at) = patch.updated_at {
        next.insert("updatedAt".to_owned(), Value::from(updated_at));
    }
    if let Some(status) = &patch.status {
        next.insert("status".to_owned(), Value::from(status.clone()));
    }
    apply_field(&mut next, "lastError", &patch.last_error);
    apply_field(&mut next, "target", &patch.target);
    write_record(
        connection,
        WriteRecord {
            meta: next,
            pinned: state.pinned,
            archived: state.archived,
            deleted: state.deleted,
            title_overridden: state.title_overridden,
            write_unread_at: false,
            searchable_text: None,
        },
    )
    .map(Some)
}

/// TS `clearTaskUnreadIfMatches`：`(meta, cleared)`。
pub fn clear_task_unread_if_matches(
    connection: &Connection,
    task: &TaskRef<'_>,
    expected_unread_at: i64,
) -> Result<(Value, bool), TasksIndexError> {
    transaction(connection, || {
        let state = read_state(connection, task)?
            .filter(|state| !state.deleted)
            .ok_or_else(|| not_found(task.task_id))?;
        if state.meta.get("unreadAt").and_then(Value::as_i64) != Some(expected_unread_at) {
            return Ok((Value::Object(state.meta), false));
        }
        let mut next = state.meta;
        next.remove("unreadAt");
        let persisted = write_record(
            connection,
            WriteRecord {
                meta: next,
                pinned: state.pinned,
                archived: state.archived,
                deleted: false,
                title_overridden: state.title_overridden,
                write_unread_at: true,
                searchable_text: None,
            },
        )?;
        Ok((persisted, true))
    })
}

/// TS `deleteArchivedTask`：不存在/已删除/未归档 → `None`（跳过，不复活也不误删）。
pub fn delete_archived_task(
    connection: &Connection,
    task: &TaskRef<'_>,
) -> Result<Option<Value>, TasksIndexError> {
    transaction(connection, || {
        let Some(state) =
            read_state(connection, task)?.filter(|state| !state.deleted && state.archived)
        else {
            return Ok(None);
        };
        let persisted = write_record(
            connection,
            WriteRecord {
                meta: state.meta,
                pinned: state.pinned,
                archived: true,
                deleted: true,
                title_overridden: state.title_overridden,
                write_unread_at: false,
                searchable_text: None,
            },
        )?;
        delete_grouping_references(connection, &state.workspace_key, &state.task_id)?;
        Ok(Some(persisted))
    })
}
