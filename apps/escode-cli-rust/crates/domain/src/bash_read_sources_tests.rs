use super::*;
use crate::json_order::Json;

/// 语料由 TS oracle 生成（bash_read_sources_corpus.gen.ts）：读文件命令识别与回填片段逐项一致。
#[test]
fn matches_ts_oracle_corpus() {
    let corpus = Json::parse(include_str!("bash_read_sources_corpus.json")).unwrap();
    let content = corpus.get("content").and_then(Json::as_str).unwrap();
    let number = |value: Option<&Json>| match value {
        Some(Json::Number(n)) => n.as_u64().map(|n| n as usize),
        _ => None,
    };
    for case in corpus.get("cases").and_then(Json::as_array).unwrap() {
        let command = case.get("command").and_then(Json::as_str).unwrap();
        let expected: Vec<Source> = case
            .get("sources")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .map(|s| Source {
                file_path: s.get("filePath").and_then(Json::as_str).unwrap().to_owned(),
                start_line: number(s.get("startLine")),
                end_line: number(s.get("endLine")),
                tail_lines: number(s.get("tailLines")),
                requires_exit_zero: matches!(s.get("requiresExitZero"), Some(Json::Bool(true))),
            })
            .collect();
        let actual = collect(command);
        assert_eq!(actual, expected, "{command}");
        let selected: Vec<Option<String>> = case
            .get("selected")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect();
        let ours: Vec<Option<String>> = actual.iter().map(|s| select(content, s).map(|sel| sel.content)).collect();
        assert_eq!(ours, selected, "{command} selected");
    }
}

#[test]
fn write_markers() {
    assert!(is_write_command("npx prettier --write ."));
    assert!(is_write_command("cargo fmt --all"));
    assert!(is_write_command("pnpm format"));
    assert!(!is_write_command("cargo build"));
}
