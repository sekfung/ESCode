//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-dwf-journal-corpus.mjs）：
//! 用 TS 建出的同一份 DDL/行重建库，再用 Rust 的读面跑同一批查询。
//! 覆盖物理→逻辑状态解码（stopped/errored/interrupted、信封嗅探失败退化成 user）、
//! 产物归并（同 id 版本、失败发布、预置看板 itemCount、primary 排序）与截断探测。
use crate::dwf_journal as store;
use crate::domain::{dwf_journal as journal, json_order::Json};
use crate::domain::workflow_run_list;
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};

const CORPUS: &str = include_str!("../tests/fixtures/dwf_journal_corpus.json");

fn corpus() -> Json {
    Json::parse(CORPUS).unwrap()
}

fn insert_rows(conn: &Connection, table: &str, rows: &[Json]) {
    for row in rows {
        let Json::Object(columns) = row else { panic!("行必须是对象") };
        let names = columns.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>();
        let values = columns
            .iter()
            .map(|(_, value)| match value {
                Json::Null => SqlValue::Null,
                Json::String(text) => SqlValue::Text(text.clone()),
                Json::Number(number) => SqlValue::Integer(number.as_i64().unwrap_or_default()),
                other => SqlValue::Text(other.compact()),
            })
            .collect::<Vec<_>>();
        conn.execute(
            &format!(
                "insert into {table} ({}) values ({})",
                names.join(", "),
                names.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
            ),
            params_from_iter(values),
        )
        .unwrap();
    }
}

/// 语料里的 params → 与 core 同一条查询：`scope: "global"` 不按 cwd 过滤，否则字面匹配 workspacePath；
/// `limit + 1` 是截断探测行。
fn query(corpus: &Json, case: &Json) -> journal::RunQuery {
    let params = case.get("params").unwrap();
    let global = params.get("scope").and_then(Json::as_str) == Some("global");
    journal::RunQuery {
        cwd: if global {
            None
        } else {
            Some(corpus.get("cwd").unwrap().as_str().unwrap().to_owned())
        },
        name: params.get("name").and_then(Json::as_str).map(str::to_owned),
        limit: params
            .get("limit")
            .and_then(|value| match value {
                Json::Number(number) => number.as_i64(),
                _ => None,
            })
            .unwrap_or(50)
            + 1,
    }
}

#[test]
fn workflow_runs_reads_match_ts() {
    let corpus = corpus();
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open(dir.path().join("corpus.sqlite")).unwrap();
    for statement in corpus.get("ddl").unwrap().as_array().unwrap() {
        conn.execute_batch(statement.as_str().unwrap()).unwrap();
    }
    let rows = corpus.get("rows").unwrap();
    insert_rows(&conn, "dwf_run", rows.get("dwf_run").unwrap().as_array().unwrap());
    insert_rows(&conn, "dwf_node", rows.get("dwf_node").unwrap().as_array().unwrap());

    for case in corpus.get("cases").unwrap().as_array().unwrap() {
        let expected = case.get("result").unwrap();
        let query = query(&corpus, case);
        let limit = (query.limit - 1) as usize;
        let found = store::journal_runs(&conn, &query).unwrap();
        let (projected, truncated) = journal::protocol_page(&found, limit);
        let actual =
            serde_json::from_str::<serde_json::Value>(&journal::to_value(&Json::Array(projected)).to_string())
                .unwrap();
        let expected_runs = serde_json::from_str::<serde_json::Value>(
            &expected.get("runs").unwrap().compact(),
        )
        .unwrap();
        assert_eq!(actual, expected_runs, "{case:?}");
        assert_eq!(
            truncated,
            expected.get("truncated") == Some(&Json::Bool(true)),
            "{case:?}"
        );
    }
}

/// `ListWorkflowRuns`（第 6 期）：同一份 journal、注册表为空的读面——Rust 的存储读 + 列表投影
/// 与 TS 的端口（内省实现）+ 工具 handler 逐字/逐值一致。
#[test]
fn workflow_run_list_matches_ts() {
    let corpus = corpus();
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open(dir.path().join("corpus.sqlite")).unwrap();
    for statement in corpus.get("ddl").unwrap().as_array().unwrap() {
        conn.execute_batch(statement.as_str().unwrap()).unwrap();
    }
    let rows = corpus.get("rows").unwrap();
    insert_rows(&conn, "dwf_run", rows.get("dwf_run").unwrap().as_array().unwrap());
    insert_rows(&conn, "dwf_node", rows.get("dwf_node").unwrap().as_array().unwrap());
    let owner = corpus.get("owner").unwrap().as_str().unwrap();

    for case in corpus.get("listCases").unwrap().as_array().unwrap() {
        let port = case.get("portInput").unwrap();
        let query = journal::RunQuery {
            cwd: port.get("cwd").and_then(Json::as_str).map(str::to_owned),
            name: None,
            limit: port.get("limit").and_then(|value| match value {
                Json::Number(number) => number.as_i64(),
                _ => None,
            })
            .unwrap(),
        };
        let limit = (query.limit - 1) as usize;
        let found = store::journal_runs(&conn, &query).unwrap();
        let truncated = found.len() > limit;
        let page = if truncated { &found[..limit] } else { &found[..] };
        let items = page
            .iter()
            .map(|entry| workflow_run_list::item(&entry.row, owner))
            .collect::<Vec<_>>();
        let output = workflow_run_list::output(&items, truncated);
        let expected = case.get("result").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&journal::to_value(&output).to_string()).unwrap(),
            serde_json::from_str::<serde_json::Value>(&expected.compact()).unwrap(),
            "{case:?}"
        );
        assert_eq!(
            workflow_run_list::format_model_content(&output),
            case.get("modelContent").unwrap().as_str().unwrap(),
            "{case:?}"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &journal::to_value(&workflow_run_list::display(&output)).to_string()
            )
            .unwrap(),
            serde_json::from_str::<serde_json::Value>(
                &case.get("display").unwrap().compact()
            )
            .unwrap(),
            "{case:?}"
        );
    }
}
