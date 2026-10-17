//! `escode-task` 的批量归档（TS `TaskIndexRepo.archiveStaleTasks` / 适配器 `archiveWorkspaceTasks`）。

use rusqlite::Connection;
use serde_json::Value;

use super::write_state::transaction;
use super::{TASK_ROW_COLUMNS, TasksIndexError, meta, read_task_row, workspace_key};

/// TS `archiveStaleTasks(params)`。
pub struct StaleArchiveParams<'a> {
    pub workspace_path: &'a str,
    pub workspace_identity: Option<&'a str>,
    pub older_than_days: i64,
    pub provider: Option<&'a str>,
}

pub(super) fn archive_stale(
    connection: &Connection,
    params: &StaleArchiveParams<'_>,
) -> Result<Vec<Value>, TasksIndexError> {
    let days = params.older_than_days.max(1);
    let cutoff = now_ms().saturating_sub(days.saturating_mul(86_400_000));
    let key = workspace_key(params.workspace_path, params.workspace_identity);
    let mut sql = format!(
        "SELECT {TASK_ROW_COLUMNS} FROM tasks
         WHERE workspace_key = ?1 AND deleted = 0 AND archived = 0 AND pinned = 0
           AND unread_at IS NULL AND updated_at < ?2 AND task_status = 'completed'"
    );
    let mut bindings = vec![
        rusqlite::types::Value::Text(key),
        rusqlite::types::Value::Integer(cutoff),
    ];
    if let Some(provider) = params.provider {
        sql.push_str(" AND provider = ?3");
        bindings.push(rusqlite::types::Value::Text(provider.to_owned()));
    }
    sql.push_str(" ORDER BY updated_at DESC, created_at DESC, task_id DESC");
    let rows = select(connection, &sql, bindings)?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    transaction(connection, || {
        for row in &rows {
            connection.execute(
                "UPDATE tasks SET archived = 1 WHERE workspace_key = ?1 AND task_id = ?2",
                rusqlite::params![
                    row["workspace_key"].as_str().unwrap_or_default(),
                    row["task_id"].as_str().unwrap_or_default()
                ],
            )?;
        }
        Ok(())
    })?;
    Ok(rows.iter().filter_map(meta::to_task_meta).collect())
}

/// TS 适配器 `archiveWorkspaceTasks`：provider=glm、未归档的全部置为归档，再返回归档列表。
pub(super) fn archive_workspace(
    connection: &Connection,
    workspace_path: &str,
    workspace_identity: Option<&str>,
) -> Result<Vec<Value>, TasksIndexError> {
    let key = workspace_key(workspace_path, workspace_identity);
    let rows = select(
        connection,
        &format!(
            "SELECT {TASK_ROW_COLUMNS} FROM tasks
             WHERE workspace_key = ?1 AND deleted = 0 AND archived = 0 AND provider = 'glm'
             ORDER BY updated_at DESC, created_at DESC, task_id DESC"
        ),
        vec![rusqlite::types::Value::Text(key.clone())],
    )?;
    if !rows.is_empty() {
        transaction(connection, || {
            for row in &rows {
                connection.execute(
                    "UPDATE tasks SET archived = 1 WHERE workspace_key = ?1 AND task_id = ?2",
                    rusqlite::params![
                        row["workspace_key"].as_str().unwrap_or_default(),
                        row["task_id"].as_str().unwrap_or_default()
                    ],
                )?;
            }
            Ok(())
        })?;
    }
    let archived = select(
        connection,
        &format!(
            "SELECT {TASK_ROW_COLUMNS} FROM tasks
             WHERE workspace_key = ?1 AND deleted = 0 AND archived = 1 AND provider = 'glm'
             ORDER BY updated_at DESC, created_at DESC, task_id DESC"
        ),
        vec![rusqlite::types::Value::Text(key)],
    )?;
    Ok(archived.iter().filter_map(meta::to_task_meta).collect())
}

fn select(
    connection: &Connection,
    sql: &str,
    bindings: Vec<rusqlite::types::Value>,
) -> Result<Vec<Value>, TasksIndexError> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(bindings), read_task_row)?;
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

impl super::TasksIndex {
    /// TS `TaskIndexRepo.archiveStaleTasks`：只归档「完成、未置顶、无未读、够旧」的任务。
    pub fn archive_stale_tasks(
        &self,
        params: &StaleArchiveParams<'_>,
    ) -> Result<Vec<Value>, TasksIndexError> {
        archive_stale(&self.connection, params)
    }

    /// TS 适配器 `archiveWorkspaceTasks`：workspace 下全部未归档任务一次性归档。
    pub fn archive_workspace_tasks(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) -> Result<Vec<Value>, TasksIndexError> {
        archive_workspace(&self.connection, workspace_path, workspace_identity)
    }
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
