//! 语料由 scripts/generate-zcode-cli-rust-webfetch-corpus.mjs 以 TS 为 oracle 生成。

use super::*;
use serde_json::Value;

fn corpus() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/webfetch_corpus.json")).unwrap()
}

fn outcome<T: ToString>(result: Result<T, String>) -> Value {
    match result {
        Ok(value) => serde_json::json!({ "ok": value.to_string() }),
        Err(error) => serde_json::json!({ "error": error }),
    }
}

#[test]
fn urls_match_ts() {
    for case in corpus()["urlCases"].as_array().unwrap() {
        let input = match &case["input"] {
            Value::String(text) => text.clone(),
            summary => format!(
                "{}{}",
                summary["prefix"].as_str().unwrap(),
                summary["repeat"]
                    .as_str()
                    .unwrap()
                    .repeat(summary["count"].as_u64().unwrap() as usize)
            ),
        };
        let normalized = normalize_url(&input);
        assert_eq!(
            outcome(normalized.clone()),
            case["normalized"],
            "normalize {input}"
        );
        let egress = normalized.ok().and_then(|url| egress_block(&url));
        assert_eq!(
            egress.map(Value::from).unwrap_or(Value::Null),
            case["egress"],
            "egress {input}"
        );
    }
}

#[test]
fn redirects_match_ts() {
    for case in corpus()["redirectCases"].as_array().unwrap() {
        let base = Url::parse(case["base"].as_str().unwrap()).unwrap();
        let location = case["location"].as_str().unwrap();
        let resolved = resolve_redirect(location, &base);
        assert_eq!(
            outcome(resolved.clone()),
            case["resolved"],
            "resolve {location}"
        );
        let permitted = resolved.ok().map(|to| redirect_permitted(&base, &to));
        assert_eq!(
            permitted.map(Value::from).unwrap_or(Value::Null),
            case["permitted"],
            "permitted {base} -> {location}"
        );
    }
}

#[test]
fn content_extraction_matches_ts() {
    use base64::Engine;
    for case in corpus()["contentCases"].as_array().unwrap() {
        let body = match case["bodyBase64"].as_str() {
            Some(encoded) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            None => case["body"].as_str().unwrap().as_bytes().to_vec(),
        };
        let content_type = case["contentType"].as_str().unwrap();
        assert_eq!(
            outcome(extract_readable(&body, content_type)),
            case["result"],
            "{content_type} {:?}",
            case["body"]
        );
    }
}

#[test]
fn truncation_matches_ts() {
    for case in corpus()["truncateCases"].as_array().unwrap() {
        let input = &case["input"];
        let text = format!(
            "{}{}",
            input["fill"]
                .as_str()
                .unwrap()
                .repeat(input["count"].as_u64().unwrap() as usize),
            input["tail"].as_str().unwrap()
        );
        let (content, truncated) = truncate_for_model(&text);
        let units: Vec<u16> = content.encode_utf16().collect();
        let head: Vec<u16> = case["head"].as_str().unwrap().encode_utf16().collect();
        let tail: Vec<u16> = case["tail"].as_str().unwrap().encode_utf16().collect();
        assert_eq!(units.len() as u64, case["length"].as_u64().unwrap());
        assert_eq!(&units[..head.len()], &head[..]);
        assert_eq!(&units[units.len() - tail.len()..], &tail[..]);
        assert_eq!(truncated, case["truncated"].as_bool().unwrap());
    }
}

#[test]
fn processing_matches_ts() {
    for case in corpus()["processingCases"].as_array().unwrap() {
        let content = match &case["content"] {
            Value::String(text) => text.clone(),
            summary => summary["repeat"]
                .as_str()
                .unwrap()
                .repeat(summary["count"].as_u64().unwrap() as usize),
        };
        let preapproved = case["preapprovedUrl"].as_bool().unwrap();
        let content_type = case["contentType"].as_str().unwrap();
        let result = &case["result"];
        if returns_markdown_directly(preapproved, content_type, &content) {
            assert!(case["prompt"].is_null());
            assert_eq!(result["result"], content);
            assert_eq!(result["truncated"], false);
            continue;
        }
        let (body, truncated) = truncate_for_model(&content);
        let prompt = processing_prompt(&body, "What is it?", preapproved);
        match &case["prompt"] {
            Value::String(expected) => assert_eq!(&prompt, expected),
            summary => {
                let units: Vec<u16> = prompt.encode_utf16().collect();
                let head: Vec<u16> = summary["head"].as_str().unwrap().encode_utf16().collect();
                let tail: Vec<u16> = summary["tail"].as_str().unwrap().encode_utf16().collect();
                assert_eq!(units.len() as u64, summary["length"].as_u64().unwrap());
                assert_eq!(&units[..head.len()], &head[..]);
                assert_eq!(&units[units.len() - tail.len()..], &tail[..]);
            }
        }
        assert_eq!(
            MAX_PROCESSING_OUTPUT_TOKENS.min(3000) as u64,
            case["maxOutputTokens"].as_u64().unwrap()
        );
        assert_eq!(truncated, result["truncated"].as_bool().unwrap());
        assert_eq!(
            processing_result(case["modelText"].as_str().unwrap()),
            result["result"].as_str().unwrap()
        );
    }
}
