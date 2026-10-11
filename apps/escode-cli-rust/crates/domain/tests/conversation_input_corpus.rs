//! 差分：Rust conversation_input 与 TS withConversationQuotes 逐字一致
//! （语料由 scripts/generate-escode-cli-rust-conversation-input-corpus.mjs 从 TS 生成）。
use serde_json::Value;
use escode_cli_domain::conversation_input::with_conversation_material;

#[test]
fn rust_conversation_material_matches_ts() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/conversation_input_corpus.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let got = with_conversation_material(
            case["text"].as_str().unwrap(),
            &case["quotes"],
            &case["botSource"],
        );
        assert_eq!(got, case["expected"].as_str().unwrap(), "{}", case["name"]);
    }
}
