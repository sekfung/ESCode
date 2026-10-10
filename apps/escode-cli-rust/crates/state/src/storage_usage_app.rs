//! 应用级用量查询（TS repositories/usage.ts `queryAppUsage`）：`v4/usage/stats` 的原始聚合，
//! 快照构造（热力图、连续天数等）在 domain::usage_stats。
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const DAY_MS: i64 = 86_400_000;

/// `{op: "app", since, until, tzOffsetMs}`。
pub(super) fn query(conn: &Connection, request: &Value) -> Result<Value> {
    let since = request["since"].as_i64().unwrap_or(0);
    let until = request["until"].as_i64().unwrap_or(i64::MAX);
    let offset = request["tzOffsetMs"].as_i64().unwrap_or(0);
    let range = params![since, until];
    let totals = conn.query_row(
        "SELECT COALESCE(SUM(computed_total_tokens),0), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
            COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM(cache_creation_input_tokens),0),
            COALESCE(SUM(cache_read_input_tokens),0), COUNT(*),
            COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END),0), AVG(time_to_first_token_ms)
         FROM rust_model_usage WHERE started_at >= ?1 AND started_at <= ?2",
        range,
        |r| {
            Ok(json!({
                "totalTokens": r.get::<_, i64>(0)?, "inputTokens": r.get::<_, i64>(1)?,
                "outputTokens": r.get::<_, i64>(2)?, "reasoningTokens": r.get::<_, i64>(3)?,
                "cacheCreationTokens": r.get::<_, i64>(4)?, "cacheReadTokens": r.get::<_, i64>(5)?,
                "modelRequestCount": r.get::<_, i64>(6)?, "modelErrorCount": r.get::<_, i64>(7)?,
                "avgTimeToFirstTokenMs": r.get::<_, Option<f64>>(8)?,
            }))
        },
    )?;
    let mut turn_totals = conn.query_row(
        "SELECT COUNT(DISTINCT session_id), COUNT(*), AVG(CASE WHEN status = 'completed' THEN duration_ms ELSE NULL END)
         FROM rust_turn_usage WHERE started_at >= ?1 AND started_at <= ?2",
        range,
        |r| {
            Ok(json!({
                "totalSessions": r.get::<_, i64>(0)?, "totalTurns": r.get::<_, i64>(1)?,
                "avgTurnDurationMs": r.get::<_, Option<f64>>(2)?,
            }))
        },
    )?;
    turn_totals["longestSessionMs"] = conn
        .query_row(
            "SELECT COALESCE(MAX(d),0) FROM (SELECT COALESCE(SUM(CASE WHEN status = 'completed' THEN duration_ms ELSE 0 END),0) AS d
             FROM rust_turn_usage WHERE started_at >= ?1 AND started_at <= ?2 GROUP BY session_id)",
            range,
            |r| r.get::<_, i64>(0),
        )?
        .into();
    let tool_totals = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END),0)
         FROM rust_tool_usage WHERE started_at >= ?1 AND started_at <= ?2",
        range,
        |r| Ok(json!({ "toolCallCount": r.get::<_, i64>(0)?, "toolErrorCount": r.get::<_, i64>(1)? })),
    )?;
    let models = rows(
        conn,
        "SELECT model_id, COALESCE(SUM(computed_total_tokens),0) AS t, COALESCE(SUM(input_tokens),0),
            COALESCE(SUM(output_tokens),0), COUNT(*)
         FROM rust_model_usage WHERE started_at >= ?1 AND started_at <= ?2 GROUP BY model_id ORDER BY t DESC",
        params![since, until],
        |r| {
            Ok(json!({
                "modelId": r.get::<_, Option<String>>(0)?, "totalTokens": r.get::<_, i64>(1)?,
                "inputTokens": r.get::<_, i64>(2)?, "outputTokens": r.get::<_, i64>(3)?,
                "requestCount": r.get::<_, i64>(4)?,
            }))
        },
    )?;
    let tools = rows(
        conn,
        "SELECT tool_name, COUNT(*) AS c, COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END),0), AVG(duration_ms)
         FROM rust_tool_usage WHERE started_at >= ?1 AND started_at <= ?2 GROUP BY tool_name ORDER BY c DESC",
        params![since, until],
        |r| {
            Ok(json!({
                "toolName": r.get::<_, String>(0)?, "callCount": r.get::<_, i64>(1)?,
                "errorCount": r.get::<_, i64>(2)?, "avgDurationMs": r.get::<_, Option<f64>>(3)?,
            }))
        },
    )?;
    // 三类按日统计合并到同一 dayIndex（本地午夜按 tzOffset 折算）。
    let mut days: BTreeMap<i64, [i64; 3]> = BTreeMap::new();
    for (column, table, aggregate) in [
        (
            0,
            "rust_model_usage",
            "COALESCE(SUM(computed_total_tokens),0)",
        ),
        (1, "rust_turn_usage", "COUNT(*)"),
        (2, "rust_tool_usage", "COUNT(*)"),
    ] {
        let sql = format!(
            "SELECT CAST((started_at + ?1) / ?2 AS INTEGER) AS d, {aggregate} FROM {table}
             WHERE started_at >= ?3 AND started_at <= ?4 GROUP BY d"
        );
        for (day, value) in rows(conn, &sql, params![offset, DAY_MS, since, until], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            days.entry(day).or_default()[column] = value;
        }
    }
    let days: Vec<Value> = days
        .into_iter()
        .map(|(day, [tokens, turns, tool_calls])| {
            json!({ "dayIndex": day, "totalTokens": tokens, "turnCount": turns, "toolCallCount": tool_calls })
        })
        .collect();
    let day_models = rows(
        conn,
        "SELECT CAST((started_at + ?1) / ?2 AS INTEGER) AS d, model_id, COALESCE(SUM(computed_total_tokens),0)
         FROM rust_model_usage WHERE started_at >= ?3 AND started_at <= ?4 GROUP BY d, model_id",
        params![offset, DAY_MS, since, until],
        |r| {
            Ok(json!({
                "dayIndex": r.get::<_, i64>(0)?, "modelId": r.get::<_, Option<String>>(1)?,
                "totalTokens": r.get::<_, i64>(2)?,
            }))
        },
    )?;
    Ok(json!({
        "totals": totals, "turnTotals": turn_totals, "toolTotals": tool_totals, "models": models,
        "tools": tools, "days": days, "dayModels": day_models,
    }))
}

fn rows<T>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare(sql)?;
    let collected = statement
        .query_map(params, map)?
        .collect::<std::result::Result<_, _>>()?;
    Ok(collected)
}
