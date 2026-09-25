//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-title-corpus.mjs）。
use super::session_title;
use serde_json::Value;

fn corpus() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/title_corpus.json")).unwrap()
}

#[test]
fn system_prompt_matches_ts() {
    assert_eq!(
        session_title::SYSTEM_PROMPT,
        corpus()["systemPrompt"].as_str().unwrap()
    );
}

#[test]
fn first_input_titles_match_ts() {
    for case in corpus()["firstInput"].as_array().unwrap() {
        assert_eq!(
            session_title::title_from_input(case["input"].as_str().unwrap()),
            case["title"].as_str().unwrap(),
            "{:?}",
            case["input"]
        );
    }
}

#[test]
fn normalized_inputs_match_ts() {
    for case in corpus()["normalized"].as_array().unwrap() {
        assert_eq!(
            session_title::normalize_title_input(case["input"].as_str().unwrap()),
            case["normalized"].as_str().unwrap(),
            "{:?}",
            case["input"]
        );
    }
}

#[test]
fn short_guard_matches_ts() {
    for case in corpus()["shortGuard"].as_array().unwrap() {
        let normalized = case["normalized"].as_str().unwrap();
        assert_eq!(
            normalized.chars().count() as u64,
            case["codePoints"].as_u64().unwrap(),
            "{:?}",
            case["input"]
        );
        assert_eq!(
            session_title::passes_short_guard(normalized),
            case["passes"].as_bool().unwrap(),
            "{:?}",
            case["input"]
        );
    }
}

#[test]
fn cleaned_titles_match_ts() {
    for case in corpus()["cleaned"].as_array().unwrap() {
        assert_eq!(
            session_title::clean_generated_title(case["raw"].as_str().unwrap()).as_deref(),
            case["title"].as_str(),
            "{:?}",
            case["raw"]
        );
    }
}
