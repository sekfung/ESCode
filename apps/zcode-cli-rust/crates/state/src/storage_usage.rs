//! 模型用量（TS session-store `model_usage` 与 repositories/usage.ts）：模型层每次逻辑请求一条事实，
//! `v4/conversation/usage` 按会话聚合（queryTaskUsage 的增量输入基线口径）。
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS rust_model_usage(
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL, turn_id TEXT, query_source TEXT NOT NULL,
            provider_id TEXT NOT NULL, model_id TEXT NOT NULL, status TEXT NOT NULL,
            started_at INTEGER NOT NULL, completed_at INTEGER, duration_ms INTEGER,
            time_to_first_token_ms INTEGER, tool_call_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_input_tokens INTEGER NOT NULL DEFAULT 0, provider_total_tokens INTEGER,
            computed_total_tokens INTEGER NOT NULL DEFAULT 0, retry_count INTEGER NOT NULL DEFAULT 0,
            error_code TEXT);
        CREATE INDEX IF NOT EXISTS rust_model_usage_session ON rust_model_usage(session_id, started_at);
        CREATE INDEX IF NOT EXISTS rust_model_usage_started ON rust_model_usage(started_at, provider_id, model_id);
        CREATE TABLE IF NOT EXISTS rust_turn_usage(session_id TEXT NOT NULL, turn_id TEXT NOT NULL, status TEXT NOT NULL,
            started_at INTEGER NOT NULL, completed_at INTEGER, duration_ms INTEGER, PRIMARY KEY(session_id, turn_id));
        CREATE INDEX IF NOT EXISTS rust_turn_usage_started ON rust_turn_usage(started_at);
        CREATE TABLE IF NOT EXISTS rust_tool_usage(session_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, turn_id TEXT,
            tool_name TEXT NOT NULL, status TEXT NOT NULL, started_at INTEGER NOT NULL, completed_at INTEGER,
            duration_ms INTEGER, PRIMARY KEY(session_id, tool_call_id));
        CREATE INDEX IF NOT EXISTS rust_tool_usage_started ON rust_tool_usage(started_at, tool_name);",
    )?;
    Ok(())
}

/// `{op: "record", fact}` 或 `{op: "task", sessionId}`。
pub(super) fn handle(conn: &Connection, request: &Value) -> Result<Value> {
    match request["op"].as_str() {
        Some("record") => record(conn, &request["fact"]).map(|_| json!({ "ok": true })),
        Some("task") => task(conn, request["sessionId"].as_str().unwrap_or_default()),
        Some("turn") | Some("tool") => span(conn, request).map(|_| json!({ "ok": true })),
        Some("app") => super::storage_usage_app::query(conn, request),
        other => anyhow::bail!("Unsupported usage request: {other:?}"),
    }
}

fn int(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|v| v as i64))
        .unwrap_or(0)
        .max(0)
}

fn record(conn: &Connection, fact: &Value) -> Result<()> {
    let input = int(&fact["inputTokens"]);
    let output = int(&fact["outputTokens"]);
    let creation = int(&fact["cacheCreationTokens"]);
    let read = int(&fact["cacheReadTokens"]);
    // TS recordModelUsage：computed = 归一化输入侧（input，缺席时 cache 之和）+ output。
    let input_side = if input > 0 { input } else { creation + read };
    conn.execute(
        "INSERT OR REPLACE INTO rust_model_usage(id,session_id,turn_id,query_source,provider_id,model_id,status,
            started_at,completed_at,duration_ms,time_to_first_token_ms,tool_call_count,input_tokens,output_tokens,
            reasoning_tokens,cache_creation_input_tokens,cache_read_input_tokens,provider_total_tokens,
            computed_total_tokens,retry_count,error_code)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
        params![
            fact["id"].as_str(),
            fact["sessionId"].as_str(),
            fact["turnId"].as_str(),
            fact["querySource"].as_str().unwrap_or("main_turn"),
            fact["providerId"].as_str().unwrap_or_default(),
            fact["modelId"].as_str().unwrap_or_default(),
            fact["status"].as_str().unwrap_or("completed"),
            int(&fact["startedAt"]),
            fact["completedAt"].as_i64(),
            fact["durationMs"].as_i64(),
            fact["timeToFirstTokenMs"].as_i64(),
            int(&fact["toolCallCount"]),
            input,
            output,
            int(&fact["reasoningTokens"]),
            creation,
            read,
            fact["providerTotalTokens"].as_i64(),
            input_side + output,
            int(&fact["retryCount"]),
            fact["errorCode"].as_str(),
        ],
    )?;
    Ok(())
}

/// 回合 / 工具的一条用量事实（TS upsertTurnUsage / upsertToolUsage）。
fn span(conn: &Connection, request: &Value) -> Result<()> {
    let started = int(&request["startedAt"]);
    let completed = int(&request["completedAt"]);
    let duration = (completed - started).max(0);
    let status = request["status"].as_str().unwrap_or("completed");
    if request["op"] == "turn" {
        conn.execute(
            "INSERT OR REPLACE INTO rust_turn_usage VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                request["sessionId"].as_str(),
                request["turnId"].as_str(),
                status,
                started,
                completed,
                duration
            ],
        )?;
    } else {
        conn.execute(
            "INSERT OR REPLACE INTO rust_tool_usage VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                request["sessionId"].as_str(),
                request["toolCallId"].as_str(),
                request["turnId"].as_str(),
                request["toolName"].as_str().unwrap_or_default(),
                status,
                started,
                completed,
                duration
            ],
        )?;
    }
    Ok(())
}

/// TS queryTaskUsage。
fn task(conn: &Connection, session: &str) -> Result<Value> {
    let mut statement = conn.prepare(
        "SELECT query_source,status,input_tokens,output_tokens,reasoning_tokens,cache_creation_input_tokens,
            cache_read_input_tokens,computed_total_tokens,provider_total_tokens
         FROM rust_model_usage WHERE session_id = ?1 ORDER BY started_at ASC, id ASC",
    )?;
    let rows = statement
        .query_map([session], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                [
                    r.get::<_, i64>(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ],
                r.get::<_, Option<i64>>(8)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (mut total, mut input_sum, mut output_sum, mut reasoning_sum) = (0i64, 0i64, 0i64, 0i64);
    let (mut creation_sum, mut read_sum, mut errors) = (0i64, 0i64, 0i64);
    let mut baseline: Map<String, Value> = Map::new();
    for (source, status, [input, output, reasoning, creation, read, computed], provider_total) in
        &rows
    {
        let raw_total = provider_total.unwrap_or(*computed).max(0);
        let input_side = stored_input_side(*input, *output, *creation, *read, raw_total);
        let tracked = matches!(source.as_str(), "main_turn" | "subagent" | "workflow_child");
        let incremental = if tracked {
            let prior = baseline.get(source).and_then(Value::as_i64).unwrap_or(0);
            // 压缩会让后续 context input 变小：累计不回扣，基线降到压缩后的值。
            baseline.insert(source.clone(), input_side.into());
            (input_side - prior).max(0)
        } else {
            input_side
        };
        total += incremental + (raw_total - input_side).max(0);
        input_sum += incremental;
        output_sum += output;
        reasoning_sum += reasoning;
        if !tracked {
            creation_sum += creation;
            read_sum += read;
        }
        if status == "error" {
            errors += 1;
        }
    }
    Ok(json!({
        "sessionId": session, "totalTokens": total, "inputTokens": input_sum, "outputTokens": output_sum,
        "reasoningTokens": reasoning_sum, "cacheCreationTokens": creation_sum, "cacheReadTokens": read_sum,
        "modelRequestCount": rows.len(), "modelErrorCount": errors, "inputBaselineBySource": baseline,
    }))
}

/// TS inputSideTokensFromStoredUsage：input 已是总输入时不再叠加缓存；按总量与哪种口径更接近判定。
fn stored_input_side(input: i64, output: i64, creation: i64, read: i64, total: i64) -> i64 {
    let cache = creation + read;
    if input <= 0 {
        return cache;
    }
    if cache <= 0 {
        return input;
    }
    if total > 0 && (total - (input + cache + output)).abs() < (total - (input + output)).abs() {
        return input + cache;
    }
    input
}

/// TS 库导入（数据迁移）：把已导入到本 workspace 的会话的 `model_usage` 行带过来，会话用量与应用统计不断档。
/// 旧库没有该表（0010 之前的版本）时跳过；重复导入按 id 去重。
pub(super) fn import_legacy(tx: &Connection, snapshot: &Connection, workspace: &str) -> Result<()> {
    let has_table: bool = snapshot.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='model_usage')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(());
    }
    let mut sessions = tx.prepare("SELECT id FROM rust_session WHERE workspace = ?1")?;
    let sessions: std::collections::HashSet<String> = sessions
        .query_map([workspace], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut rows = snapshot.prepare(
        "SELECT id,session_id,turn_id,query_source,provider_id,model_id,status,started_at,completed_at,duration_ms,
            time_to_first_token_ms,tool_call_count,input_tokens,output_tokens,reasoning_tokens,
            cache_creation_input_tokens,cache_read_input_tokens,provider_total_tokens,computed_total_tokens,
            retry_count,error_code FROM model_usage",
    )?;
    let mut insert = tx.prepare(
        "INSERT OR IGNORE INTO rust_model_usage VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
    )?;
    let mut cursor = rows.query([])?;
    while let Some(row) = cursor.next()? {
        let session: String = row.get(1)?;
        if !sessions.contains(&session) {
            continue;
        }
        let values: Vec<rusqlite::types::Value> = (0..21)
            .map(|i| row.get::<_, rusqlite::types::Value>(i))
            .collect::<std::result::Result<_, _>>()?;
        insert.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(id: &str, source: &str, input: i64, output: i64) -> Value {
        json!({"id": id, "sessionId": "s1", "querySource": source, "providerId": "p", "modelId": "m",
            "status": "completed", "startedAt": id.len(), "inputTokens": input, "outputTokens": output})
    }

    #[test]
    fn task_usage_counts_main_turn_input_incrementally() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        record(&conn, &fact("a", "main_turn", 1000, 50)).unwrap();
        record(&conn, &fact("ab", "main_turn", 1300, 40)).unwrap();
        record(&conn, &fact("abc", "session_title", 200, 5)).unwrap();
        let usage = task(&conn, "s1").unwrap();
        assert_eq!(usage["inputTokens"], 1500);
        assert_eq!(usage["outputTokens"], 95);
        assert_eq!(usage["totalTokens"], 1595);
        assert_eq!(usage["modelRequestCount"], 3);
        assert_eq!(usage["inputBaselineBySource"]["main_turn"], 1300);
    }

    #[test]
    fn legacy_import_copies_rows_of_imported_sessions_only() {
        let dest = Connection::open_in_memory().unwrap();
        ensure_schema(&dest).unwrap();
        dest.execute_batch("CREATE TABLE rust_session(workspace TEXT, id TEXT, body TEXT); INSERT INTO rust_session VALUES('w','s1','{}');").unwrap();
        let source = Connection::open_in_memory().unwrap();
        ensure_schema(&source).unwrap();
        record(&source, &fact("a", "main_turn", 10, 1)).unwrap();
        source.execute_batch("ALTER TABLE rust_model_usage RENAME TO model_usage; UPDATE model_usage SET session_id='s1';
            INSERT INTO model_usage(id,session_id,query_source,provider_id,model_id,status,started_at) VALUES('b','other','main_turn','p','m','completed',1);").unwrap();
        import_legacy(&dest, &source, "w").unwrap();
        let count: i64 = dest
            .query_row("SELECT COUNT(*) FROM rust_model_usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
