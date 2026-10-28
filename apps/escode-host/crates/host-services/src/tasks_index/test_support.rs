//! `tasks_index` 测试共用夹具：按生成资产建库、期望账本。
#![allow(dead_code)] // 各测试文件只用其中一部分。

use super::schema;
use rusqlite::Connection;

/// 夹具：按生成资产建一个空库（表/索引/迁移账本齐全），用于打开与对账测试。
pub(super) fn fixture(ledger: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    let connection = Connection::open(&path).unwrap();
    super::fixture::apply_generated_schema(&connection, ledger).unwrap();
    dir
}

pub(super) fn expected_ledger() -> Vec<(&'static str, &'static str)> {
    schema::EXPECTED_MIGRATIONS.to_vec()
}

pub(super) fn expected_project_ledger() -> Vec<(&'static str, &'static str)> {
    schema::EXPECTED_PROJECT_MIGRATIONS.to_vec()
}
