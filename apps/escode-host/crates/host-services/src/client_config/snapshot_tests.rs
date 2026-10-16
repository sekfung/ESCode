use serde_json::Value;

use super::*;

/// TS oracle（`scripts/generate-escode-host-client-config-fixtures.mjs`）：真实 `parseClientConfigSnapshot` 对信封求值。
const GOLDEN: &str = include_str!("snapshot_golden.json");

#[test]
fn every_payload_is_decided_like_the_real_ts_parser() {
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden parses");
    assert_eq!(golden["fixtureVersion"], 1);
    let cases = golden["cases"].as_array().expect("cases");
    assert!(cases.len() > 90, "语料规模异常: {}", cases.len());
    let mut mismatches = Vec::new();
    for case in cases {
        let actual = match parse_snapshot(&case["payload"]) {
            Ok(snapshot) => serde_json::json!({"ok": true, "snapshot": snapshot.to_json()}),
            Err(message) => serde_json::json!({"ok": false, "message": message}),
        };
        if actual != case["expected"] {
            mismatches.push(format!(
                "payload={}\n  TS  ={}\n  Rust={actual}",
                case["payload"], case["expected"]
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "与 TS 判定不一致 ({} 条):\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn dynamic_workflow_folding_matches_the_real_ts_function_on_every_combination() {
    use super::dynamic_workflow::resolve;
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden parses");
    let cases = golden["dynamicWorkflow"].as_array().expect("cases");
    assert!(cases.len() > 1000, "语料规模异常: {}", cases.len());
    let mut mismatches = Vec::new();
    for case in cases {
        let input = &case["input"];
        // 缺省 = 键不存在（对应 JS 的 `undefined`）。
        let actual =
            resolve(input.get("remote"), input.get("env"), input.get("userMode")).to_json();
        if actual != case["expected"] {
            mismatches.push(format!(
                "input={input}\n  TS  ={}\n  Rust={actual}",
                case["expected"]
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "与 TS 判定不一致 ({} 条):\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
