//! 语料由 scripts/generate-zcode-cli-rust-saved-workflow-corpus.mjs 以 TS frontmatter 编解码为 oracle 生成。

use super::*;

const CORPUS: &str = include_str!("../tests/fixtures/saved_workflow_corpus.json");

fn corpus() -> Json {
    Json::parse(CORPUS).unwrap()
}

#[test]
fn serialization_matches_ts_byte_for_byte() {
    let corpus = corpus();
    for case in corpus.get("serialized").and_then(Json::as_array).unwrap() {
        let meta = case.get("meta").unwrap();
        let expected = case.get("file").and_then(Json::as_str).unwrap();
        let actual = serialize(meta, "export default async () => {\n  return 1;\n}\n");
        assert_eq!(actual, expected, "{}", meta.compact());
    }
}

#[test]
fn parsing_matches_ts() {
    let corpus = corpus();
    for case in corpus.get("parsed").and_then(Json::as_array).unwrap() {
        let source = case.get("source").and_then(Json::as_str).unwrap();
        let result = case.get("result").unwrap();
        match parse(source) {
            Ok(parsed) => {
                assert_eq!(result.get("ok"), Some(&Json::Bool(true)), "{source:?}");
                assert_eq!(parsed.meta.compact(), result.get("meta").unwrap().compact(), "{source:?}");
                assert_eq!(Some(parsed.script.as_str()), result.get("script").and_then(Json::as_str), "{source:?}");
                assert_eq!(
                    Json::Number(parsed.body_line_offset.into()).compact(),
                    result.get("bodyLineOffset").unwrap().compact(),
                    "{source:?}"
                );
            }
            Err(failure) => {
                assert_eq!(result.get("ok"), Some(&Json::Bool(false)), "{source:?}: {failure:?}");
                assert_eq!(Some(failure.reason), result.get("reason").and_then(Json::as_str), "{source:?}");
                // YAML 解析器的错误措辞不同（规格记录）；其余原因逐字比对。
                if failure.reason != "invalid_yaml" {
                    assert_eq!(Some(failure.detail.as_str()), result.get("detail").and_then(Json::as_str), "{source:?}");
                }
            }
        }
    }
}
