//! `tasks_index` 测试共用夹具：按生成资产建库、期望账本。
#![allow(dead_code)] // 各测试文件只用其中一部分。

use super::schema;
use rusqlite::Connection;

/// 夹具：按生成资产建一个空库（表/索引/迁移账本齐全），用于打开与对账测试。
pub(super) fn fixture(ledger: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    let connection = Connection::open(&path).unwrap();
    for statement in schema::MIGRATED_DDL {
        connection.execute_batch(statement).unwrap();
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks_schema_migration (
               id TEXT PRIMARY KEY, checksum TEXT NOT NULL, time_applied INTEGER NOT NULL)",
        )
        .unwrap();
    for (id, checksum) in ledger {
        connection
            .execute(
                "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
                rusqlite::params![id, checksum],
            )
            .unwrap();
    }
    dir
}

pub(super) fn expected_ledger() -> Vec<(&'static str, &'static str)> {
    schema::EXPECTED_MIGRATIONS.to_vec()
}
