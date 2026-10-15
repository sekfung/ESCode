//! tasks-index（`<dataBaseDir>/.escode/v2/tasks-index.sqlite`）的 Host 侧访问层。
//!
//! 与 TS Host（`node:sqlite`）**同库互操作**：同一文件、同一迁移账本、同一物理 schema。
//! 本模块负责：
//! 1. 打开既有库（`busy_timeout = 25`、`foreign_keys = ON`，与 TS `prepareTasksIndexStorage` 同）；
//! 2. **迁移账本对账**：库内 `tasks_schema_migration` 必须与生成资产
//!    （`schema.rs`，由真实 TS 迁移产物提取）逐条一致——缺失/多出/checksum 不符都拒绝服务，
//!    不猜、不半迁移（对应 docs/specs/rust-u2-tasks-index.md §3）；
//! 3. 读取任务投影（`getTaskMeta` 同 TS `TaskIndexRepo`）。
//!
//! 写路径（`createSession` 等）在后续切片落地前一律不开放：读写分离保证「索引是运行时的投影」
//! 这一不变式不会因为没有 syncer 而被破坏。

mod meta;
pub mod query;
mod query_grouped;
mod query_shared;
mod query_snippets;
pub mod schema;
mod write;
pub mod write_state;

pub use write::SyncOptions;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 索引层错误：可分类、可回报（不吞成字符串）。
#[derive(Debug)]
pub enum TasksIndexError {
    /// 库文件不存在：TS Host 尚未初始化 tasks-index（Host 不自行建库）。
    Missing { path: PathBuf },
    /// 迁移账本与生成资产不一致（缺失/多出/checksum 不符）。
    MigrationMismatch {
        missing: Vec<String>,
        unknown: Vec<String>,
        mismatched: Vec<String>,
    },
    /// SQLite 层错误。
    Sqlite(rusqlite::Error),
    /// 生成资产自校验失败（手改过 `schema.rs`）。
    AssetTampered { what: &'static str },
    /// 状态写的目标任务不存在或已删除（TS `updateTaskState` 抛出的同一错误）。
    TaskNotFound { task_id: String },
}

impl std::fmt::Display for TasksIndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(
                formatter,
                "tasks-index is missing at {} (the TS Host must initialize it first)",
                path.display()
            ),
            Self::MigrationMismatch {
                missing,
                unknown,
                mismatched,
            } => write!(
                formatter,
                "tasks-index migration ledger mismatch: missing={missing:?} unknown={unknown:?} mismatched={mismatched:?}"
            ),
            Self::Sqlite(error) => write!(formatter, "tasks-index sqlite error: {error}"),
            Self::AssetTampered { what } => {
                write!(formatter, "generated tasks-index {what} asset is tampered")
            }
            // 文案与 TS `updateTaskState` 一致（Renderer 只展示）。
            Self::TaskNotFound { task_id } => {
                write!(formatter, "task index 中不存在 task: {task_id}")
            }
        }
    }
}

impl std::error::Error for TasksIndexError {}

impl From<rusqlite::Error> for TasksIndexError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// 打开并校验既有库（只读语义：本切片不提供任何写入）。
#[derive(Debug)]
pub struct TasksIndex {
    connection: Connection,
}

impl TasksIndex {
    /// 打开 `<path>`；文件不存在或账本不符即报错（不建库、不迁移）。
    pub fn open(path: &Path) -> Result<Self, TasksIndexError> {
        verify_assets()?;
        if !path.is_file() {
            return Err(TasksIndexError::Missing {
                path: path.to_path_buf(),
            });
        }
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        // 与 TS `prepareTasksIndexStorage` 同参数：小 busy_timeout、外键开。
        connection.pragma_update(None, "busy_timeout", 25)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let index = Self { connection };
        let ledger = index.migration_ledger()?;
        let (missing, unknown, mismatched) = compare_ledger(&ledger);
        if !missing.is_empty() || !unknown.is_empty() || !mismatched.is_empty() {
            return Err(TasksIndexError::MigrationMismatch {
                missing,
                unknown,
                mismatched,
            });
        }
        Ok(index)
    }

    /// 库内迁移账本（按 id 升序）。
    pub fn migration_ledger(&self) -> Result<Vec<(String, String)>, TasksIndexError> {
        let mut statement = self
            .connection
            .prepare("SELECT id, checksum FROM tasks_schema_migration ORDER BY id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut ledger = Vec::new();
        for row in rows {
            ledger.push(row?);
        }
        Ok(ledger)
    }

    /// TS `TaskIndexRepo.seedTaskMetaIfMissing`：不存在才写（存在则原样返回行投影）。
    pub fn seed_task_meta_if_missing(
        &self,
        meta_object: &serde_json::Map<String, Value>,
    ) -> Result<Value, TasksIndexError> {
        write::seed_task_meta_if_missing(&self.connection, meta_object)
    }

    /// TS `TaskIndexRepo.syncTaskMeta`：带合并规则的 upsert（快照/agent 补丁写入路径）。
    pub fn sync_task_meta(
        &self,
        meta_object: &serde_json::Map<String, Value>,
        options: &write::SyncOptions,
    ) -> Result<Value, TasksIndexError> {
        write::sync_task_meta(&self.connection, meta_object, options)
    }

    /// TS `updateTaskState`。
    pub fn update_task_state(
        &self,
        task: &write_state::TaskRef<'_>,
        patch: &write_state::StatePatch,
    ) -> Result<Value, TasksIndexError> {
        write_state::update_task_state(&self.connection, task, patch)
    }

    /// TS `applyAgentPatch`：行缺失/已删回 `None`（不抛错）。
    pub fn apply_agent_patch(
        &self,
        task: &write_state::TaskRef<'_>,
        patch: &write_state::AgentPatch,
    ) -> Result<Option<Value>, TasksIndexError> {
        write_state::apply_agent_patch(&self.connection, task, patch)
    }

    /// TS `clearTaskUnreadIfMatches`。
    pub fn clear_task_unread_if_matches(
        &self,
        task: &write_state::TaskRef<'_>,
        expected_unread_at: i64,
    ) -> Result<(Value, bool), TasksIndexError> {
        write_state::clear_task_unread_if_matches(&self.connection, task, expected_unread_at)
    }

    /// 测试读原始列（行投影不含 pinned 等标记）。
    #[cfg(test)]
    pub(super) fn connection_for_tests(&self) -> &rusqlite::Connection {
        &self.connection
    }

    /// TS `deleteArchivedTask`。
    pub fn delete_archived_task(
        &self,
        task: &write_state::TaskRef<'_>,
    ) -> Result<Option<Value>, TasksIndexError> {
        write_state::delete_archived_task(&self.connection, task)
    }

    /// TS `TaskIndexRepo.getTaskMeta`：缺失或 `deleted = 1` 回 `None`。
    pub fn get_task_meta(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        task_id: &str,
    ) -> Result<Option<Value>, TasksIndexError> {
        let workspace_key = workspace_key(workspace_path, workspace_identity);
        let mut statement = self.connection.prepare(&format!(
            "SELECT {TASK_ROW_COLUMNS} FROM tasks
             WHERE workspace_key = ?1 AND task_id = ?2 AND deleted = 0"
        ))?;
        let row = statement
            .query_row(rusqlite::params![workspace_key, task_id], read_task_row)
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(row.and_then(|row| meta::to_task_meta(&row)))
    }

    /// TS `TaskIndexRepo.listTaskMetas`：按可选过滤读任务列表。
    /// 排序与 TS 同：`updated_at DESC, created_at DESC, task_id DESC`（SQLite TEXT 即字节序）。
    pub fn list_task_metas(
        &self,
        filters: &TaskFilters<'_>,
    ) -> Result<Vec<Value>, TasksIndexError> {
        let mut sql = format!("SELECT {TASK_ROW_COLUMNS} FROM tasks WHERE 1 = 1");
        let mut bindings: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(workspace_path) = filters.workspace_path {
            let key = workspace_key(workspace_path, filters.workspace_identity);
            sql.push_str(&format!(" AND workspace_key = ?{}", bindings.len() + 1));
            bindings.push(rusqlite::types::Value::Text(key));
        }
        if !filters.include_deleted {
            sql.push_str(" AND deleted = 0");
        }
        if let Some(provider) = filters.provider {
            sql.push_str(&format!(" AND provider = ?{}", bindings.len() + 1));
            bindings.push(rusqlite::types::Value::Text(provider.to_owned()));
        }
        for (column, value) in [("pinned", filters.pinned), ("archived", filters.archived)] {
            if let Some(value) = value {
                sql.push_str(&format!(" AND {column} = ?{}", bindings.len() + 1));
                bindings.push(rusqlite::types::Value::Integer(i64::from(value)));
            }
        }
        sql.push_str(" ORDER BY updated_at DESC, created_at DESC, task_id DESC");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(bindings), read_task_row)?;
        let mut metas = Vec::new();
        for row in rows {
            if let Some(meta) = meta::to_task_meta(&row?) {
                metas.push(meta);
            }
        }
        Ok(metas)
    }

    /// TS `TaskIndexRepo.queryTaskList`：多 workspace 聚合 + 可选全文搜索 + 分页。
    pub fn query_task_list(
        &self,
        query: &query::TaskListQuery<'_>,
    ) -> Result<Value, TasksIndexError> {
        query::query_task_list(&self.connection, query)
    }

    /// TS `TaskIndexRepo.queryGroupedTaskViewStructure`：group/成员/顶层排序原始结构。
    pub fn query_grouped_task_view_structure(
        &self,
        workspace_scopes: &[(&str, Option<&str>)],
    ) -> Result<Value, TasksIndexError> {
        query_grouped::query_grouped_task_view_structure(&self.connection, workspace_scopes)
    }

    /// TS `TaskIndexRepo.listDeletedTaskIds`：`deleted = 1` 的 id，按 id 升序。
    pub fn list_deleted_task_ids(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        provider: Option<&str>,
    ) -> Result<Vec<String>, TasksIndexError> {
        let mut sql =
            String::from("SELECT task_id FROM tasks WHERE workspace_key = ?1 AND deleted = 1");
        if provider.is_some() {
            sql.push_str(" AND provider = ?2");
        }
        sql.push_str(" ORDER BY task_id");
        let workspace_key = workspace_key(workspace_path, workspace_identity);
        let mut statement = self.connection.prepare(&sql)?;
        let map_row = |row: &rusqlite::Row<'_>| row.get::<_, String>(0);
        let rows = match provider {
            Some(provider) => statement
                .query_map(rusqlite::params![workspace_key, provider], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            None => statement
                .query_map(rusqlite::params![workspace_key], map_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }
}

pub use query::TaskFilters;

/// 读一行任务投影（列顺序见 `TASK_ROW_COLUMNS`）。
pub(super) fn read_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(serde_json::json!({
        "workspace_key": row.get::<_, String>(0)?,
        "workspace_path": row.get::<_, String>(1)?,
        "workspace_identity": row.get::<_, Option<String>>(2)?,
        "task_id": row.get::<_, String>(3)?,
        "title": row.get::<_, String>(4)?,
        "task_status": row.get::<_, Option<String>>(5)?,
        "provider": row.get::<_, Option<String>>(6)?,
        "mode": row.get::<_, String>(7)?,
        "model": row.get::<_, Option<String>>(8)?,
        "migration_source": row.get::<_, Option<String>>(9)?,
        "forked_from_task_id": row.get::<_, Option<String>>(10)?,
        "cron_automation_id": row.get::<_, Option<String>>(11)?,
        "off_peak_task_id": row.get::<_, Option<String>>(12)?,
        "created_at": row.get::<_, i64>(13)?,
        "updated_at": row.get::<_, i64>(14)?,
        "unread_at": row.get::<_, Option<i64>>(15)?,
        "title_overridden": row.get::<_, i64>(16)?,
        "meta_json": row.get::<_, String>(17)?,
        // `searchable_text` 只供 `queryTaskList` 的摘要窗口用（TS 同样在行里取它）。
        "searchable_text": row.get::<_, String>(18)?,
    }))
}

/// 行归属键：`workspaceIdentity?.trim() || workspacePath`（AGENTS.md Workspace Identity）。
pub fn workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| workspace_path.to_owned(), str::to_owned)
}

/// TS `TaskIndexRepo` 的 `getTaskMeta` 投影列（顺序即索引）。
pub(super) const TASK_ROW_COLUMNS: &str =
    "workspace_key, workspace_path, workspace_identity, task_id, title,
    task_status, provider, mode, model, migration_source, forked_from_task_id, cron_automation_id,
    off_peak_task_id, created_at, updated_at, unread_at, title_overridden, meta_json,
    searchable_text";
/// `TASK_ROW_COLUMNS` 的列数（追加列从这个下标起读）。
pub(super) const TASK_ROW_COLUMN_COUNT: usize = 19;

/// 生成资产自校验：hash 与清单一致，防止手改 `schema.rs` 绕过 TS 对账。
fn verify_assets() -> Result<(), TasksIndexError> {
    let canonical = schema::EXPECTED_MIGRATIONS
        .iter()
        .map(|(id, checksum)| format!("{id}\t{checksum}"))
        .collect::<Vec<_>>()
        .join("\n");
    if hex(&Sha256::digest(canonical.as_bytes())) != schema::EXPECTATIONS_HASH {
        return Err(TasksIndexError::AssetTampered { what: "migrations" });
    }
    if hex(&Sha256::digest(schema::MIGRATED_DDL.join("\n").as_bytes())) != schema::DDL_HASH {
        return Err(TasksIndexError::AssetTampered { what: "ddl" });
    }
    Ok(())
}

/// 账本差异三分类（与生成资产逐条比对）。
fn compare_ledger(ledger: &[(String, String)]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for (id, checksum) in schema::EXPECTED_MIGRATIONS {
        match ledger.iter().find(|(current, _)| current == id) {
            Some((_, actual)) if actual == checksum => {}
            Some(_) => mismatched.push((*id).to_owned()),
            None => missing.push((*id).to_owned()),
        }
    }
    let unknown = ledger
        .iter()
        .filter(|(id, _)| {
            !schema::EXPECTED_MIGRATIONS
                .iter()
                .any(|(known, _)| known == id)
        })
        .map(|(id, _)| id.clone())
        .collect();
    (missing, unknown, mismatched)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "write_state_tests.rs"]
mod write_state_tests;
#[cfg(test)]
#[path = "write_tests.rs"]
mod write_tests;
