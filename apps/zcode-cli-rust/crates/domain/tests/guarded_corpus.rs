//! 差分：Rust guarded::match_dangerous_command 与 TS matchDangerousCommand 逐条一致
//! （语料由 scripts/generate-zcode-cli-rust-guarded-corpus.mjs 从 TS 生成）。
use serde_json::{Value, json};
use zcode_cli_domain::guarded::{Dialect, Outcome, match_dangerous_command};

#[test]
fn rust_guarded_matcher_matches_ts_corpus() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/guarded_corpus.json")).unwrap();
    let mut failures = vec![];
    let cases = fixture["cases"].as_array().unwrap();
    for case in cases {
        let command = case["command"].as_str().unwrap();
        let dialect = Dialect::parse(case["dialect"].as_str().unwrap()).unwrap();
        let got = match match_dangerous_command(command, dialect) {
            Outcome::Matched {
                rule_id,
                start,
                end,
            } => json!({"status":"matched","ruleId":rule_id,"start":start,"end":end}),
            Outcome::NotMatched => json!({"status":"notMatched"}),
            Outcome::Unsupported => json!({"status":"unsupported"}),
        };
        if got != case["result"] {
            failures.push(format!(
                "{command:?} [{}]: TS {} / Rust {got}",
                case["dialect"], case["result"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} / {} guarded cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
