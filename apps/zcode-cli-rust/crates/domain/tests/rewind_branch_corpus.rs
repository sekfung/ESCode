//! 差分：Rust active_branch 与 TS selectActiveConversationBranch 逐条一致。
use serde_json::Value;
use zcode_cli_domain::rewind_branch::active_branch;

#[test]
fn rust_matches_ts_active_conversation_branch() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/rewind_branch_corpus.json")).unwrap();
    let ids: Vec<String> = fixture["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() > 100, "corpus too small");
    let mismatches: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let want: Vec<usize> = serde_json::from_value(case[1].clone()).unwrap();
            let got = active_branch(&ids, &case[0]);
            (got != want).then(|| format!("{}: {got:?} != {want:?}", case[0]))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first: {:#?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(5)]
    );
}
