//! Rust Host 启动时补齐 project-storage schema 与账本（docs/specs/rust-project-storage.md §1）。
//!
//! 背景：Node 时代的 project-storage 迁移实现已随 W3 移出本仓，Rust Host 是新库的唯一所有者；
//! Main 只负责用 TS `prepareTasksIndexStorage` 准备 tasks 账本。这里为**全新的空 tasks 库**
//! 补上 project-storage 的物理对象与 `schema_migration` 账本（DDL/checksum 来自生成资产）。
//!
//! 安全边界：只处理 `tasks` 为空且项目表/账本都不存在的库；已有任务但没有项目账本的历史库
//! 必须显式拒绝——那需要真正的数据迁移（legacy settings/任务归位），不能用空 schema 假装完成。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::{TasksIndexError, schema, verify_assets};

const PROJECT_TABLES: &[&str] = &[
    "hosts",
    "projects",
    "project_roots",
    "remote_project_entries",
];

/// 为空的 tasks 库补齐项目 schema/账本；已有项目账本时直接返回（由 `TasksIndex::open` 校验）。
pub fn ensure_project_schema(path: &Path) -> Result<(), TasksIndexError> {
    verify_assets()?;
    if !path.is_file() {
        return Err(TasksIndexError::Missing {
            path: path.to_path_buf(),
        });
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.pragma_update(None, "busy_timeout", 5000)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    if project_ledger_count(&connection)? > 0 {
        return Ok(());
    }
    if !table_exists(&connection, "tasks")? {
        return Err(project_mismatch("tasks ledger is missing"));
    }
    let tasks: i64 = connection.query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))?;
    if tasks > 0 {
        return Err(project_mismatch(
            "tasks already exist; legacy project migration is required",
        ));
    }
    for table in PROJECT_TABLES {
        if table_exists(&connection, table)? {
            return Err(project_mismatch("project tables exist without a ledger"));
        }
    }

    connection.execute_batch("BEGIN IMMEDIATE")?;
    let applied = (|| -> Result<(), TasksIndexError> {
        // 取锁后复查：另一进程可能已经完成 bootstrap。
        if project_ledger_count(&connection)? > 0 {
            return Ok(());
        }
        for statement in schema::PROJECT_DDL {
            connection.execute_batch(statement)?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migration (
               id text primary key, checksum text not null, app_version text, applied_at integer not null)",
        )?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or_default();
        let mut insert = connection.prepare(
            "INSERT INTO schema_migration (id, checksum, app_version, applied_at)
             VALUES (?1, ?2, NULL, ?3)",
        )?;
        for (id, checksum) in schema::EXPECTED_PROJECT_MIGRATIONS {
            insert.execute(rusqlite::params![id, checksum, now])?;
        }
        Ok(())
    })();
    match applied {
        Ok(()) => {
            connection.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn table_exists(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
}

fn project_ledger_count(connection: &Connection) -> rusqlite::Result<i64> {
    let exists: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migration'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Ok(0);
    }
    connection.query_row("SELECT count(*) FROM schema_migration", [], |row| {
        row.get(0)
    })
}

fn project_mismatch(reason: &str) -> TasksIndexError {
    TasksIndexError::MigrationMismatch {
        missing: schema::EXPECTED_PROJECT_MIGRATIONS
            .iter()
            .map(|(id, _)| format!("project:{id}"))
            .collect(),
        unknown: Vec::new(),
        mismatched: vec![reason.to_owned()],
    }
}
