use super::*;

/// 语料由 TS oracle 生成（tool_input_validation_corpus.gen.ts）：问题对象与模型可见文案逐字一致。
#[test]
fn matches_ts_oracle_corpus() {
    let corpus = Json::parse(include_str!("tool_input_validation_corpus.json")).unwrap();
    let mut checked = 0;
    for suite in corpus.as_array().unwrap() {
        let schema = suite.get("schema").unwrap();
        for case in suite.get("cases").and_then(Json::as_array).unwrap() {
            let input = case.get("input").unwrap();
            let issues = validate(input, schema);
            let label = input.compact();
            assert_eq!(Json::Array(issues.clone()).compact(), case.get("issues").unwrap().compact(), "{label}");
            let expected = case.get("content").and_then(Json::as_str);
            let actual = (!issues.is_empty()).then(|| model_content("mcp__demo__run", &issues));
            assert_eq!(actual.as_deref(), expected, "{label}");
            checked += 1;
        }
    }
    assert_eq!(checked, 18);
}

#[test]
fn empty_schema_accepts_anything() {
    assert!(validate(&Json::Null, &Json::object()).is_empty());
}
