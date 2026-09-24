//! 差分：Rust bash_policy::is_readonly 与 TS isRuntimeReadOnlyBashCommand 在 5k+ 条语料上逐条一致。
//! 失败时区分方向：Rust 放宽（TS 非只读而 Rust 只读）是阻断缺陷。
use serde_json::Value;
use zcode_cli_domain::bash_policy::is_readonly;

#[test]
fn rust_bash_readonly_matches_ts_corpus() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/bash_readonly_corpus.json")).unwrap();
    let expected = fixture["readOnly"].as_str().unwrap().as_bytes();
    let (mut looser, mut stricter) = (vec![], vec![]);
    for (i, command) in fixture["commands"].as_array().unwrap().iter().enumerate() {
        let command = command.as_str().unwrap();
        let want = expected[i] == b'1';
        let got = is_readonly(command);
        if got && !want {
            looser.push(command);
        } else if !got && want {
            stricter.push(command);
        }
    }
    assert!(
        looser.is_empty() && stricter.is_empty(),
        "looser (Rust readonly, TS not) {}: {:#?}\nstricter {}: {:#?}",
        looser.len(),
        &looser[..looser.len().min(15)],
        stricter.len(),
        &stricter[..stricter.len().min(15)]
    );
}
