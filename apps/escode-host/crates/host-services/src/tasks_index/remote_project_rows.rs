//! `remote-projects` 入口表的行读取与 JSON 投影辅助。
//!
//! 与上游 `services/src/remote-projects/index.ts` 的行映射保持一致：
//! 数据库列名到 Renderer 字段名的转换只在这里维护。

use anyhow::{Result, bail};
use rusqlite::Connection;
use serde_json::{Map, Value, json};

use super::project_read::now_ms;

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

pub(super) fn required_text(value: &Value, key: &str, code: &str) -> Result<String> {
    let value = text(value, key);
    if value.trim().is_empty() {
        bail!("{code}");
    }
    Ok(value)
}

pub(super) fn next_version(previous: Option<i64>) -> i64 {
    let now = now_ms();
    previous.map_or(now, |previous| now.max(previous.saturating_add(1)))
}

fn row_object(row: &rusqlite::Row<'_>) -> Result<Map<String, Value>> {
    let mut object = Map::new();
    for index in 0..row.as_ref().column_count() {
        let name = row.as_ref().column_name(index)?.to_owned();
        let value = match row.get_ref(index)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(value) => json!(value),
            rusqlite::types::ValueRef::Real(value) => json!(value),
            rusqlite::types::ValueRef::Text(value) => {
                json!(String::from_utf8_lossy(value).into_owned())
            }
            rusqlite::types::ValueRef::Blob(value) => json!(value),
        };
        object.insert(name, value);
    }
    Ok(object)
}

pub(super) fn select_all(
    connection: &Connection,
    sql: &str,
    bindings: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Value>> {
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query(bindings)?;
    let mut values = Vec::new();
    while let Some(row) = rows.next()? {
        values.push(Value::Object(row_object(row)?));
    }
    Ok(values)
}

pub(super) fn select_optional(
    connection: &Connection,
    sql: &str,
    bindings: &[&dyn rusqlite::ToSql],
) -> Result<Option<Value>> {
    Ok(select_all(connection, sql, bindings)?.into_iter().next())
}

pub(super) fn parse_json_column(row: &Value, key: &str) -> Result<Value> {
    let raw = row[key].as_str().unwrap_or_default();
    serde_json::from_str(raw).map_err(|error| anyhow::anyhow!("invalid {key}: {error}"))
}

pub(super) fn host_from_row(row: &Value) -> Result<Value> {
    let mut host = Map::new();
    host.insert("id".to_owned(), row["host_id"].clone());
    host.insert("membershipId".to_owned(), row["membership_id"].clone());
    host.insert("kind".to_owned(), row["kind"].clone());
    host.insert("target".to_owned(), parse_json_column(row, "target_json")?);
    host.insert("position".to_owned(), row["position"].clone());
    for (column, field) in [
        ("last_connection_status", "lastConnectionStatus"),
        ("last_connection_error", "lastConnectionError"),
        ("last_opened_at", "lastOpenedAt"),
    ] {
        if !row[column].is_null() {
            host.insert(field.to_owned(), row[column].clone());
        }
    }
    Ok(Value::Object(host))
}

pub(super) fn entry_from_row(row: &Value) -> Result<Value> {
    let mut entry = Map::new();
    entry.insert("id".to_owned(), row["entry_id"].clone());
    entry.insert("hostId".to_owned(), row["host_id"].clone());
    entry.insert("projectId".to_owned(), row["remote_project_id"].clone());
    entry.insert("name".to_owned(), row["cached_name"].clone());
    entry.insert(
        "roots".to_owned(),
        parse_json_column(row, "cached_roots_json")?,
    );
    entry.insert(
        "legacyWorkspace".to_owned(),
        if row["legacy_workspace_json"].is_null() {
            Value::Null
        } else {
            parse_json_column(row, "legacy_workspace_json")?
        },
    );
    entry.insert("lastSyncedAt".to_owned(), row["last_synced_at"].clone());
    if !row["initial_position"].is_null() {
        entry.insert(
            "initialPosition".to_owned(),
            row["initial_position"].clone(),
        );
    }
    Ok(Value::Object(entry))
}
