use super::*;
use crate::json_order::Json;

/// 语料由 TS oracle 生成（shell_snapshot_corpus.gen.ts）：快照创建脚本、cwd 捕获包装、git-bash 路径与重置提示逐字一致。
#[test]
fn matches_ts_oracle_corpus() {
    let corpus = Json::parse(include_str!("shell_snapshot_corpus.json")).unwrap();
    let text = |value: &Json, key: &str| value.get(key).and_then(Json::as_str).unwrap().to_owned();
    for case in corpus.get("scripts").and_then(Json::as_array).unwrap() {
        let kind = ShellKind::detect(&text(case, "shell"));
        let exists = matches!(case.get("exists"), Some(Json::Bool(true)));
        let script = creation_script(exists, &text(case, "configPath"), &text(case, "pathValue"), kind, &text(case, "snapshotPath"));
        assert_eq!(script, text(case, "script"), "{} exists={exists}", text(case, "shell"));
    }
    for case in corpus.get("captures").and_then(Json::as_array).unwrap() {
        let cmd = text(case, "dialect") == "cmd";
        assert_eq!(cwd_capture("echo 'hi'\nfalse", &text(case, "cwdFile"), cmd), text(case, "command"));
    }
    for case in corpus.get("gitBash").and_then(Json::as_array).unwrap() {
        assert_eq!(git_bash_to_windows(&text(case, "value")), text(case, "windows"));
    }
    for case in corpus.get("suffix").and_then(Json::as_array).unwrap() {
        assert_eq!(reset_suffix(&text(case, "stderr"), &text(case, "root")), text(case, "result"));
    }
}
