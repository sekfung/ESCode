//! tasks-index 写路径（TS `TaskIndexRepo.writeRecord` / `seedTaskMetaIfMissing` /
//! `syncTaskMeta` 的等价实现）。
//!
//! 写入的合并规则逐条对齐 TS（这些规则决定列表顺序与状态显示，不能简化）：
//! - `updated_at` 取 `max(入库值, 已有值)`（快照可能落后于 `applyAgentPatch` 写入的时间戳）；
//! - 终态（completed/error）不被更旧的 running 快照降级（`shouldPreserveNewerTerminalStatus`）；
//! - 用户手动标题（`title_overridden`）不被 agent 标题覆盖；
//! - `migrationSource` / `cronAutomationId` / `offPeakTaskId` / `unreadAt` 缺省保留既有值；
//! - `pinned`/`archived`/`deleted` 缺省沿用既有行；
//! - `searchable_text` 只在显式传入时更新（否则保留行内现值，避免清空已索引正文）；
//! - `unread_at` 只在 `writeUnreadAt` 时覆盖；`last_unread_at` 取历史最大值（CAS 标记）。
//!
//! meta_json 的键序：TS 用 `JSON.stringify(meta)`（zod 解析顺序）。JSON 对象键序无语义，
//! 两端互相读取都按对象解析，因此这里按输入 meta 的原顺序写回（保留未知键）。

use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

use super::TasksIndexError;
use super::meta;
use super::workspace_key;

/// TS `TASK_SEARCH_TEXT_MAX_CHARS`：searchable_text 入库前截断。
const TASK_SEARCH_TEXT_MAX_CHARS: usize = 200_000;

/// `writeRecord` 的入参（TS `TaskIndexWriteRecord`）。
pub struct WriteRecord {
    pub meta: Map<String, Value>,
    pub pinned: bool,
    pub archived: bool,
    pub deleted: bool,
    pub title_overridden: bool,
    /// 只有 unread 专属写路径可以改现有行的 unread_at。
    pub write_unread_at: bool,
    /// `None` = 保留行内现值（TS 的 `undefined`）。
    pub searchable_text: Option<String>,
}

/// `syncTaskMeta` 的可选覆盖（`None` = 沿用既有行或 meta）。
#[derive(Default)]
pub struct SyncOptions {
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub deleted: Option<bool>,
    pub title_overridden: Option<bool>,
    pub searchable_text: Option<String>,
}

pub fn seed_task_meta_if_missing(
    connection: &Connection,
    meta_object: &Map<String, Value>,
) -> Result<Value, TasksIndexError> {
    if let Some(existing) = read_row(connection, meta_object)? {
        return Ok(meta::to_task_meta(&existing).unwrap_or_else(|| meta::fallback_meta(&existing)));
    }
    let title_overridden = meta_object
        .get("titleOverridden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    write_record(
        connection,
        WriteRecord {
            meta: meta_object.clone(),
            pinned: false,
            archived: false,
            deleted: false,
            title_overridden,
            write_unread_at: false,
            searchable_text: None,
        },
    )
}

pub fn sync_task_meta(
    connection: &Connection,
    incoming: &Map<String, Value>,
    options: &SyncOptions,
) -> Result<Value, TasksIndexError> {
    let existing_row = read_row(connection, incoming)?;
    let existing_meta = existing_row
        .as_ref()
        .and_then(meta::to_task_meta)
        .and_then(|value| value.as_object().cloned());
    let existing_flags = read_flags(connection, incoming)?;

    let title_overridden = options.title_overridden.unwrap_or_else(|| {
        existing_flags
            .map(|flags| flags.title_overridden)
            .unwrap_or(false)
    });
    let preserve_terminal = should_preserve_newer_terminal_status(existing_meta.as_ref(), incoming);
    let updated_at = incoming
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(
            existing_meta
                .as_ref()
                .and_then(|meta| meta.get("updatedAt"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
        );

    let mut merged = incoming.clone();
    // agent 只负责 session 核心标题；用户手动改过的标题不被后台刷新冲掉。
    if title_overridden
        && let Some(existing) = existing_meta.as_ref().and_then(|meta| meta.get("title"))
    {
        merged.insert("title".into(), existing.clone());
    }
    merged.insert("titleOverridden".into(), json!(title_overridden));
    if preserve_terminal {
        for key in ["status", "lastError"] {
            match existing_meta.as_ref().and_then(|meta| meta.get(key)) {
                Some(value) => {
                    merged.insert(key.into(), value.clone());
                }
                None => {
                    merged.remove(key);
                }
            }
        }
    }
    // 快照不带目标/来源身份时保留既有值（否则 cron 图标、导入来源会丢）。
    for key in [
        "migrationSource",
        "cronAutomationId",
        "offPeakTaskId",
        "unreadAt",
    ] {
        if merged.get(key).is_none_or(Value::is_null)
            && let Some(existing) = existing_meta.as_ref().and_then(|meta| meta.get(key))
        {
            merged.insert(key.into(), existing.clone());
        }
    }
    merged.insert("updatedAt".into(), json!(updated_at));

    write_record(
        connection,
        WriteRecord {
            meta: merged,
            pinned: options
                .pinned
                .unwrap_or_else(|| existing_flags.map(|flags| flags.pinned).unwrap_or(false)),
            archived: options
                .archived
                .unwrap_or_else(|| existing_flags.map(|flags| flags.archived).unwrap_or(false)),
            deleted: options
                .deleted
                .unwrap_or_else(|| existing_flags.map(|flags| flags.deleted).unwrap_or(false)),
            title_overridden,
            write_unread_at: false,
            searchable_text: options.searchable_text.clone(),
        },
    )
}

/// TS `writeRecord` 的 upsert（含 unread CAS 与 `last_unread_at` 单调更新）。
pub(super) fn write_record(
    connection: &Connection,
    record: WriteRecord,
) -> Result<Value, TasksIndexError> {
    let meta = &record.meta;
    let workspace_path = text(meta, "workspacePath");
    let task_id = text(meta, "taskId");
    let key = workspace_key(
        &workspace_path,
        meta.get("workspaceIdentity").and_then(Value::as_str),
    );
    // `searchable_text` 传 None 表示"不动现有值"（读一次行；否则会把已索引正文清空）。
    let searchable_text = match &record.searchable_text {
        Some(text) => text
            .chars()
            .take(TASK_SEARCH_TEXT_MAX_CHARS)
            .collect::<String>(),
        None => read_searchable_text(connection, &key, &task_id)?.unwrap_or_default(),
    };
    let unread_at = meta.get("unreadAt").and_then(Value::as_i64);
    connection
        .execute(
            "INSERT INTO tasks (
              workspace_key, workspace_path, workspace_identity, task_id, title, task_status,
              provider, mode, model, migration_source, forked_from_task_id, cron_automation_id,
              off_peak_task_id, created_at, updated_at, unread_at, last_unread_at, pinned,
              archived, deleted, title_overridden, searchable_text, meta_json
            ) VALUES (
              ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
              ?19, ?20, ?21, ?22, ?23
            )
            ON CONFLICT(workspace_key, task_id) DO UPDATE SET
              workspace_path = excluded.workspace_path,
              workspace_identity = excluded.workspace_identity,
              title = excluded.title,
              task_status = excluded.task_status,
              provider = excluded.provider,
              mode = excluded.mode,
              model = excluded.model,
              migration_source = excluded.migration_source,
              forked_from_task_id = excluded.forked_from_task_id,
              cron_automation_id = excluded.cron_automation_id,
              off_peak_task_id = excluded.off_peak_task_id,
              created_at = excluded.created_at,
              updated_at = excluded.updated_at,
              unread_at = CASE WHEN ?24 = 1 THEN excluded.unread_at ELSE tasks.unread_at END,
              last_unread_at = MAX(
                tasks.last_unread_at,
                COALESCE(tasks.unread_at, 0),
                CASE WHEN ?24 = 1 THEN excluded.last_unread_at ELSE 0 END
              ),
              pinned = excluded.pinned,
              archived = excluded.archived,
              deleted = excluded.deleted,
              title_overridden = excluded.title_overridden,
              searchable_text = excluded.searchable_text,
              meta_json = excluded.meta_json",
            params![
                key,
                workspace_path,
                meta.get("workspaceIdentity").and_then(Value::as_str),
                task_id,
                text(meta, "title"),
                meta.get("status").and_then(Value::as_str),
                meta.get("provider").and_then(Value::as_str),
                text(meta, "mode"),
                meta.get("model").and_then(Value::as_str),
                meta.get("migrationSource").and_then(Value::as_str),
                meta.get("forkedFromTaskId").and_then(Value::as_str),
                meta.get("cronAutomationId").and_then(Value::as_str),
                meta.get("offPeakTaskId").and_then(Value::as_str),
                meta.get("createdAt").and_then(Value::as_i64).unwrap_or(0),
                meta.get("updatedAt").and_then(Value::as_i64).unwrap_or(0),
                unread_at,
                unread_at.unwrap_or(0),
                i64::from(record.pinned),
                i64::from(record.archived),
                i64::from(record.deleted),
                i64::from(record.title_overridden),
                searchable_text,
                Value::Object(meta.clone()).to_string(),
                i64::from(record.write_unread_at),
            ],
        )
        .map_err(TasksIndexError::Sqlite)?;
    let persisted = read_row(connection, meta)?
        .ok_or_else(|| TasksIndexError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
    Ok(meta::to_task_meta(&persisted).unwrap_or_else(|| meta::fallback_meta(&persisted)))
}

fn text(meta: &Map<String, Value>, key: &str) -> String {
    meta.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn read_row(
    connection: &Connection,
    meta: &Map<String, Value>,
) -> Result<Option<Value>, TasksIndexError> {
    let workspace_path = text(meta, "workspacePath");
    let key = workspace_key(
        &workspace_path,
        meta.get("workspaceIdentity").and_then(Value::as_str),
    );
    let task_id = text(meta, "taskId");
    let mut statement = connection
        .prepare(&format!(
            "SELECT {columns} FROM tasks WHERE workspace_key = ?1 AND task_id = ?2",
            columns = super::TASK_ROW_COLUMNS
        ))
        .map_err(TasksIndexError::Sqlite)?;
    statement
        .query_row(params![key, task_id], super::read_task_row)
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(TasksIndexError::Sqlite(other)),
        })
}

/// 既有行的布尔标记（`writeRecord` 缺省沿用它们）。
#[derive(Clone, Copy)]
struct RowFlags {
    pinned: bool,
    archived: bool,
    deleted: bool,
    title_overridden: bool,
}

/// 取既有行的布尔标记（单独查一次：行投影不含 pinned/archived/deleted）。
fn read_flags(
    connection: &Connection,
    meta: &Map<String, Value>,
) -> Result<Option<RowFlags>, TasksIndexError> {
    let workspace_path = text(meta, "workspacePath");
    let key = workspace_key(
        &workspace_path,
        meta.get("workspaceIdentity").and_then(Value::as_str),
    );
    let task_id = text(meta, "taskId");
    let mut statement = connection
        .prepare(
            "SELECT pinned, archived, deleted, title_overridden FROM tasks
             WHERE workspace_key = ?1 AND task_id = ?2",
        )
        .map_err(TasksIndexError::Sqlite)?;
    statement
        .query_row(params![key, task_id], |row| {
            Ok(RowFlags {
                pinned: row.get::<_, i64>(0)? == 1,
                archived: row.get::<_, i64>(1)? == 1,
                deleted: row.get::<_, i64>(2)? == 1,
                title_overridden: row.get::<_, i64>(3)? == 1,
            })
        })
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(TasksIndexError::Sqlite(other)),
        })
}

fn read_searchable_text(
    connection: &Connection,
    workspace_key: &str,
    task_id: &str,
) -> Result<Option<String>, TasksIndexError> {
    let mut statement = connection
        .prepare("SELECT searchable_text FROM tasks WHERE workspace_key = ?1 AND task_id = ?2")
        .map_err(TasksIndexError::Sqlite)?;
    statement
        .query_row(params![workspace_key, task_id], |row| row.get(0))
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(TasksIndexError::Sqlite(other)),
        })
}

/// TS `shouldPreserveNewerTerminalStatus`：既有是终态且入库更旧时，不把终态降级回 running。
fn should_preserve_newer_terminal_status(
    existing_meta: Option<&Map<String, Value>>,
    incoming: &Map<String, Value>,
) -> bool {
    let Some(existing) = existing_meta else {
        return false;
    };
    let existing_status = existing.get("status").and_then(Value::as_str);
    if !matches!(existing_status, Some("completed" | "error")) {
        return false;
    }
    let incoming_status = incoming.get("status").and_then(Value::as_str);
    if matches!(incoming_status, Some(status) if !status.is_empty() && status != "running") {
        return false;
    }
    existing
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        > incoming
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0)
}
