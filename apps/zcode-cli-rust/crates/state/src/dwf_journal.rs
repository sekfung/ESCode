//! dwf_* journal 的 SQLite 读面（docs/specs/rust-dynamic-workflow.md 第 4 期前置）：DDL 与查询形状
//! 对齐 TS `session-store/migrations.ts`（`0019_dwf_journal`）与
//! `repositories/dwf-journal-{introspection,artifacts}.ts`。
//!
//! 只读：Rust 侧还没有 run 的写入方（引擎是第 4 期），这里先把「同一份库、同一套表、同一套读法」
//! 立起来，`workflows/runs` 与将来的引擎共用。
use crate::domain::dwf_journal::{ArtifactRow, JournalRun, RunQuery, RunRow};
use anyhow::Result;
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};

/// TS `0019_dwf_journal` 的四张表与索引。列形状即终态（beta 基线），Rust 库要与 TS 库对得上。
const DDL: &str = "
  create table if not exists dwf_run (
    id text primary key,
    parent_session_id text,
    cwd text,
    name text,
    script_text text,
    script_hash text,
    args_json text,
    tool_call_id text,
    resumed_from text,
    caps_max_concurrency integer not null,
    spent_tokens integer not null default 0,
    status text not null check(status in (
      'pending',
      'running',
      'completed',
      'failed',
      'cancelled'
    )),
    result_json text,
    failure_json text,
    time_created integer not null,
    time_updated integer not null
  );
  create index if not exists dwf_run_cwd_idx on dwf_run(cwd, time_updated);
  create table if not exists dwf_actor (
    id integer primary key autoincrement,
    run_id text not null references dwf_run(id) on delete cascade,
    site_id text not null,
    ordinal integer not null,
    name text,
    persona_json text,
    resolved_model text,
    session_id text,
    time_created integer not null,
    time_updated integer not null,
    unique(run_id, site_id, ordinal)
  );
  create index if not exists dwf_actor_run_idx on dwf_actor(run_id);
  create table if not exists dwf_node (
    id integer primary key autoincrement,
    run_id text not null references dwf_run(id) on delete cascade,
    site_id text not null,
    ordinal integer not null,
    kind text not null check(kind in ('ask', 'world-read', 'world-run', 'report', 'artifact')),
    actor_site_id text,
    actor_ordinal integer,
    actor_seq integer,
    input_hash text not null,
    input_json text,
    status text not null check(status in ('running', 'completed', 'failed')),
    result_json text,
    error_json text,
    stats_json text,
    message_boundary integer,
    artifact_id text,
    time_created integer not null,
    time_updated integer not null,
    unique(run_id, site_id, ordinal)
  );
  create index if not exists dwf_node_run_idx on dwf_node(run_id);
  create index if not exists dwf_node_artifact_idx on dwf_node(run_id, artifact_id);
  create table if not exists dwf_event (
    id integer primary key autoincrement,
    run_id text not null references dwf_run(id) on delete cascade,
    sequence integer not null,
    type text not null,
    payload_json text not null,
    time_created integer not null,
    unique(run_id, sequence)
  );
  create index if not exists dwf_event_artifact_idx
    on dwf_event(run_id, json_extract(payload_json, '$.artifactId'), sequence);
";

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(DDL)?;
    Ok(())
}

/// TS `listRuns`：cwd / name 下推 SQL，`time_updated desc`，`limit` **由调用方钳制**（截断探测行不许
/// 在这里被吃掉）。状态过滤 TS 支持、`workflows/runs` 不用，故未实现。
pub fn list_runs(conn: &Connection, query: &RunQuery) -> Result<Vec<RunRow>> {
    // 空状态集合的语义是「不匹配任何状态」而不是「不过滤」：显式传下来的过滤器不许静默失效。
    if query.statuses.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Vec::new());
    }
    if query.limit <= 0 {
        return Ok(Vec::new());
    }
    let mut sql = String::from(
        "select id, parent_session_id, cwd, name, script_text, resumed_from, tool_call_id, args_json, \
         spent_tokens, status, failure_json, time_created, time_updated from dwf_run where 1 = 1",
    );
    let mut values: Vec<SqlValue> = Vec::new();
    if let Some(cwd) = &query.cwd {
        sql.push_str(" and cwd = ?");
        values.push(SqlValue::Text(cwd.clone()));
    }
    if let Some(name) = &query.name {
        sql.push_str(" and name = ?");
        values.push(SqlValue::Text(name.clone()));
    }
    if let Some(statuses) = &query.statuses {
        // TS `encodeRunStatusPredicate`：`stopped` / `errored` 在物理层共享 `failed` 列值，靠
        // failure_json 的 code 在 SQL 里分清——取一页再筛会让 limit 与截断探测失真。
        let mut clauses = Vec::new();
        for status in statuses {
            match status.as_str() {
                "stopped" => {
                    clauses.push(
                        "(status = 'cancelled' or (status = 'failed' and \
                         json_extract(failure_json, '$.code') = ?))"
                            .to_owned(),
                    );
                    values.push(SqlValue::Text("Interrupted".into()));
                }
                "errored" => {
                    clauses.push(
                        "(status = 'failed' and coalesce(json_extract(failure_json, '$.code'), '') <> ?)"
                            .to_owned(),
                    );
                    values.push(SqlValue::Text("Interrupted".into()));
                }
                other => {
                    clauses.push("status = ?".to_owned());
                    values.push(SqlValue::Text(other.to_owned()));
                }
            }
        }
        sql.push_str(&format!(" and ({})", clauses.join(" or ")));
    }
    sql.push_str(" order by time_updated desc limit ?");
    values.push(SqlValue::Integer(query.limit));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(values), |row| {
        Ok(RunRow {
            id: row.get(0)?,
            parent_session_id: row.get(1)?,
            cwd: row.get(2)?,
            name: row.get(3)?,
            script_text: row.get(4)?,
            resumed_from: row.get(5)?,
            tool_call_id: row.get(6)?,
            args_json: row.get(7)?,
            spent_tokens: row.get(8)?,
            status: row.get(9)?,
            failure_json: row.get(10)?,
            time_created: row.get(11)?,
            time_updated: row.get(12)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// TS `listArtifactRows`：`kind = 'artifact'` 按插入序（版本的先后就是落库的先后）。
pub fn artifact_rows(conn: &Connection, run_id: &str) -> Result<Vec<ArtifactRow>> {
    let mut statement = conn.prepare(
        "select status, artifact_id, result_json from dwf_node \
         where run_id = ? and kind = 'artifact' order by id",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok(ArtifactRow {
            status: row.get(0)?,
            artifact_id: row.get(1)?,
            result_json: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `workflows/runs` 要的一页：run 行 + 每行的产物行与标签计数。
pub fn journal_runs(conn: &Connection, query: &RunQuery) -> Result<Vec<JournalRun>> {
    let mut runs = Vec::new();
    for row in list_runs(conn, query)? {
        runs.push(JournalRun {
            artifacts: artifact_rows(conn, &row.id)?,
            reports: report_counts(conn, &row.id)?,
            row,
        });
    }
    Ok(runs)
}

/// TS `tagItemCounts`：打了 id 标签的 `report` 行数（预置看板的数据量）。
fn report_counts(conn: &Connection, run_id: &str) -> Result<Vec<(String, i64)>> {
    let mut statement = conn.prepare(
        "select artifact_id, count(*) from dwf_node \
         where run_id = ? and kind = 'report' and artifact_id is not null group by artifact_id",
    )?;
    let rows = statement.query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
#[path = "dwf_journal_tests.rs"]
mod tests;
