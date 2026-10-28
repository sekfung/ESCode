//! 测试/互操作用：按生成资产建库并写入 tasks + project 两份迁移账本。
//! 只给夹具使用；生产路径由 Node Host/CLI 执行迁移。

use super::schema;
use rusqlite::Connection;

pub fn apply_generated_schema(
    connection: &Connection,
    tasks_ledger: &[(&str, &str)],
) -> rusqlite::Result<()> {
    for statement in schema::MIGRATED_DDL {
        connection.execute_batch(statement)?;
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks_schema_migration (
           id TEXT PRIMARY KEY, checksum TEXT NOT NULL, time_applied INTEGER NOT NULL)",
    )?;
    for (id, checksum) in tasks_ledger {
        connection.execute(
            "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
            rusqlite::params![id, checksum],
        )?;
    }
    for (id, checksum) in schema::EXPECTED_PROJECT_MIGRATIONS {
        connection.execute(
            "INSERT INTO schema_migration (id, checksum, applied_at) VALUES (?1, ?2, 0)",
            rusqlite::params![id, checksum],
        )?;
    }
    Ok(())
}
