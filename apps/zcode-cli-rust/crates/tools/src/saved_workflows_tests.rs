//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-saved-workflow-store-corpus.mjs）。
//!
//! 语料里的路径都是相对临时根目录的；OS 错误文案（EISDIR/EACCES）与 YAML 解析器措辞跨平台不同，
//! 语料只记 `kind`（以及"文件名不可用"这条固定文案），这两类只比 kind。其余逐字比对。
use super::saved_workflows as store;
use std::path::Path;
use zcode_cli_domain::{json_order::Json, saved_workflow as codec};

const CORPUS: &str = include_str!("../tests/fixtures/saved_workflow_store_corpus.json");
const SCRIPT: &str = "export default async () => {\n  return 1;\n}\n";

fn corpus() -> Json {
    Json::parse(CORPUS).unwrap()
}

fn text(value: &Json) -> &str {
    value.as_str().expect("语料里的字符串")
}

/// 语料的 `tree`：字符串是文件正文，`null` 是目录。
fn materialize(root: &Path, tree: &Json) {
    let Json::Object(entries) = tree else {
        panic!("tree 必须是对象")
    };
    for (path, content) in entries {
        let full = root.join(path);
        if matches!(content, Json::Null) {
            std::fs::create_dir_all(&full).unwrap();
        } else {
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, text(content)).unwrap();
        }
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|rest| rest.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn collect_files(dir: &Path, root: &Path, out: &mut Vec<(String, Json)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            collect_files(&path, root, out);
        } else {
            let content = std::fs::read_to_string(&path).unwrap();
            out.push((rel(root, &path), Json::str(content)));
        }
    }
}

fn snapshot(root: &Path) -> Json {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    // 目录顺序随文件系统而定，两边都按路径排序后比较。
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Json::Object(files)
}

fn kind_name(kind: store::InvalidKind) -> &'static str {
    match kind {
        store::InvalidKind::NotAWorkflowName => "not_a_workflow_name",
        store::InvalidKind::ReadError => "read_error",
        store::InvalidKind::ParseError => "parse_error",
    }
}

fn scope_of(input: &Json) -> Option<store::Scope> {
    input.get("scope").and_then(Json::as_str).map(|scope| store::Scope::parse(scope).unwrap())
}

fn reason_is(expected: &Json, reason: &str) {
    assert_eq!(expected.get("reason").and_then(Json::as_str), Some(reason), "{expected:?}");
}

fn path_is(root: &Path, expected: &Json, path: &Path) {
    assert_eq!(expected.get("path").and_then(Json::as_str), Some(rel(root, path).as_str()), "{expected:?}");
}

#[test]
fn store_reads_match_ts() {
    let corpus = corpus();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    materialize(root, corpus.get("tree").unwrap());
    let home = root.join(text(corpus.get("home").unwrap()));
    let cwd = root.join(text(corpus.get("cwd").unwrap()));
    for case in corpus.get("reads").unwrap().as_array().unwrap() {
        let op = text(case.get("op").unwrap());
        let expected = case.get("result").unwrap();
        match op {
            "list" => {
                let listed = store::list(&cwd, &home, scope_of(case));
                let entries: Vec<Json> = listed
                    .entries
                    .iter()
                    .map(|entry| {
                        let mut value = store::entry_json(entry);
                        value.set("path", Json::str(rel(root, &entry.path)));
                        value
                    })
                    .collect();
                assert_eq!(Json::Array(entries), *expected.get("entries").unwrap(), "{case:?}");
                let invalid: Vec<Json> = listed
                    .invalid
                    .iter()
                    .map(|invalid| {
                        let mut value = Json::object();
                        value.set("path", Json::str(rel(root, &invalid.path)));
                        value.set("kind", Json::str(kind_name(invalid.kind)));
                        if invalid.kind == store::InvalidKind::NotAWorkflowName {
                            value.set("reason", Json::str(&invalid.reason));
                        }
                        value
                    })
                    .collect();
                assert_eq!(Json::Array(invalid), *expected.get("invalid").unwrap(), "{case:?}");
            }
            "resolve" => {
                let input = case.get("input").unwrap();
                let resolved = store::resolve(&cwd, &home, text(input.get("name").unwrap()), scope_of(input));
                match resolved {
                    store::Resolve::Found(found) => {
                        assert_eq!(expected.get("ok"), Some(&Json::Bool(true)), "{case:?}");
                        assert_eq!(text(expected.get("name").unwrap()), found.name);
                        assert_eq!(text(expected.get("scope").unwrap()), found.scope.as_str());
                        assert_eq!(text(expected.get("path").unwrap()), rel(root, &found.path));
                        assert_eq!(*expected.get("meta").unwrap(), found.meta, "{case:?}");
                        assert_eq!(text(expected.get("script").unwrap()), found.script);
                        assert_eq!(text(expected.get("source").unwrap()), found.source);
                        assert_eq!(
                            expected.get("bodyLineOffset").unwrap(),
                            &Json::Number(found.body_line_offset.into()),
                            "{case:?}"
                        );
                    }
                    store::Resolve::InvalidName { detail } => {
                        reason_is(expected, "invalid_name");
                        assert_eq!(expected.get("detail").and_then(Json::as_str), Some(detail.as_str()), "{case:?}");
                    }
                    store::Resolve::NotFound => {
                        reason_is(expected, "not_found");
                    }
                    store::Resolve::ParseError { path, reason, detail } => {
                        reason_is(expected, "parse_error");
                        // saphyr 与 TS `yaml` 的错误措辞不同（同编解码语料）：只有非 invalid_yaml 才比 detail。
                        if reason != "invalid_yaml" {
                            assert_eq!(
                                expected.get("detail").and_then(Json::as_str),
                                Some(detail.as_str()),
                                "{case:?}"
                            );
                        }
                        path_is(root, expected, &path);
                    }
                    store::Resolve::ReadError { path, .. } => {
                        // OS 错误文案跨平台不同：只比 reason 与 path。
                        reason_is(expected, "read_error");
                        path_is(root, expected, &path);
                    }
                }
            }
            "exists" => {
                let input = case.get("input").unwrap();
                let exists = store::exists(&cwd, &home, text(input.get("name").unwrap()), scope_of(input));
                assert_eq!(expected, &Json::Bool(exists), "{case:?}");
            }
            "shadowing" => {
                let input = case.get("input").unwrap();
                let scope = scope_of(input).unwrap();
                let shadowing = store::shadowing(&cwd, &home, text(input.get("name").unwrap()), scope);
                match shadowing {
                    Some(value) => assert_eq!(expected, &Json::str(value), "{case:?}"),
                    None => assert_eq!(expected, &Json::Null, "{case:?}"),
                }
            }
            other => panic!("未知读用例 {other}"),
        }
    }
}

#[test]
fn store_mutations_match_ts() {
    for case in corpus().get("mutations").unwrap().as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        materialize(root, case.get("tree").unwrap());
        let home = root.join("home");
        let cwd = root.join("repo");
        let input = case.get("input").unwrap();
        let actual = match text(case.get("op").unwrap()) {
            "save" => {
                let meta = input.get("meta").unwrap().clone();
                let saved = store::save(
                    &cwd,
                    &home,
                    text(input.get("name").unwrap()),
                    &meta,
                    SCRIPT,
                    scope_of(input),
                )
                .unwrap();
                let mut value = Json::object();
                value.set("path", Json::str(rel(root, &saved.path)));
                value.set("scope", Json::str(saved.scope.as_str()));
                value.set("overwritten", Json::Bool(saved.overwritten));
                value
            }
            "move" => match store::move_to_project(&cwd, &home, text(input.get("name").unwrap())) {
                store::Move::Ok { from, to } => {
                    let mut value = Json::object();
                    value.set("ok", Json::Bool(true));
                    value.set("from", Json::str(rel(root, &from)));
                    value.set("to", Json::str(rel(root, &to)));
                    value
                }
                store::Move::InvalidName { .. } => failure("invalid_name"),
                store::Move::NotFound => failure("not_found"),
                store::Move::TargetExists { path } => {
                    let mut value = failure("target_exists");
                    value.set("path", Json::str(rel(root, &path)));
                    value
                }
                store::Move::ReadError { path, .. } => {
                    let mut value = failure("read_error");
                    value.set("path", Json::str(rel(root, &path)));
                    value
                }
                store::Move::WriteError { path, .. } => {
                    let mut value = failure("write_error");
                    value.set("path", Json::str(rel(root, &path)));
                    value
                }
            },
            other => panic!("未知写用例 {other}"),
        };
        assert_eq!(actual, *case.get("result").unwrap(), "{case:?}");
        assert_eq!(snapshot(root), *case.get("files").unwrap(), "{case:?}");
    }
}

fn failure(reason: &str) -> Json {
    let mut value = Json::object();
    value.set("ok", Json::Bool(false));
    value.set("reason", Json::str(reason));
    value
}

#[test]
fn workflow_args_match_ts() {
    for case in corpus().get("args").unwrap().as_array().unwrap() {
        let optional = |key: &str| {
            case.get(key)
                .filter(|value| value.is_object())
                .cloned()
        };
        let (declaration, provided) = (optional("declaration"), optional("provided"));
        let result = codec::validate_args(declaration.as_ref(), provided.as_ref());
        if case.get("ok") == Some(&Json::Bool(true)) {
            let args = result.unwrap_or_else(|errors| panic!("{case:?} 期望通过，实际 {errors:?}"));
            assert_eq!(args, *case.get("args").unwrap(), "{case:?}");
        } else {
            let errors = result.err().unwrap_or_else(|| panic!("{case:?} 期望失败，实际通过"));
            let expected: Vec<String> =
                case.get("errors").unwrap().as_array().unwrap().iter().map(|value| text(value).to_owned()).collect();
            assert_eq!(errors, expected, "{case:?}");
        }
    }
}
