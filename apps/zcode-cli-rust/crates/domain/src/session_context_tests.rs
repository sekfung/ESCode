//! 语料由 scripts/generate-zcode-cli-rust-session-context-corpus.mjs 以 TS handler 为 oracle 生成。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use serde_json::Value;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use super::*;
use crate::session_context_lite::model_content;
use crate::session_context_text::{len16, slice16, tail16};

#[derive(serde::Deserialize)]
struct Corpus<'a> {
    #[serde(borrow)]
    fixtures: BTreeMap<String, Vec<&'a RawValue>>,
    cases: Vec<Value>,
}

/// 脚本化模型的 future 都是立即就绪的，一次 poll 即完成。
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("scripted model future must be ready"),
    }
}

/// 期望值可能是长字符串摘要 {sha256, length, head, tail}。
fn assert_text(actual: &str, expected: &Value, label: &str) {
    match expected {
        Value::String(text) => assert_eq!(actual, text, "{label}"),
        digest => {
            assert_eq!(
                len16(actual) as u64,
                digest["length"].as_u64().unwrap(),
                "{label} length"
            );
            assert_eq!(
                slice16(actual, 0, 300),
                digest["head"].as_str().unwrap(),
                "{label} head"
            );
            assert_eq!(
                tail16(actual, 300),
                digest["tail"].as_str().unwrap(),
                "{label} tail"
            );
            let hash = format!("{:x}", Sha256::digest(actual.as_bytes()));
            assert_eq!(hash, digest["sha256"].as_str().unwrap(), "{label} sha256");
        }
    }
}

#[test]
fn read_session_context_matches_ts() {
    let corpus: Corpus = serde_json::from_str(include_str!(
        "../tests/fixtures/session_context_corpus.json"
    ))
    .unwrap();
    for case in &corpus.cases {
        let name = case["name"].as_str().unwrap();
        let input = Input::parse(&case["input"]).unwrap();
        let output = if case["session"].is_null() {
            not_found_output(&input)
        } else {
            let session = &case["session"];
            let info = SessionInfo {
                id: session["id"].as_str().unwrap().into(),
                title: session["title"].as_str().unwrap().into(),
                directory: session["directory"].as_str().unwrap().into(),
                path: session["path"].as_str().map(str::to_owned),
            };
            let messages: Vec<Value> = match case["fixture"].as_str().unwrap() {
                "empty" => vec![],
                key => corpus.fixtures[key]
                    .iter()
                    .map(|raw| message_from_ts_json(raw.get()).unwrap())
                    .collect(),
            };
            let mut script: Vec<Value> = case["model"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .rev()
                .collect();
            let mut calls: Vec<LiteCall> = Vec::new();
            let model_max = (!case["model"].is_null()).then_some(8000);
            let output = block_on(run(&input, &info, &messages, model_max, |call| {
                calls.push(call);
                let reply = match script.pop() {
                    Some(Value::String(text)) => Ok(text),
                    Some(error) => Err(error["error"].as_str().unwrap().to_owned()),
                    None => Ok(String::new()),
                };
                std::future::ready(reply)
            }));
            let expected = case["calls"].as_array().unwrap();
            assert_eq!(calls.len(), expected.len(), "{name} call count");
            for (index, (call, want)) in calls.iter().zip(expected).enumerate() {
                assert_eq!(
                    call.max_output_tokens as u64,
                    want["maxOutputTokens"].as_u64().unwrap(),
                    "{name} call {index} tokens"
                );
                assert_eq!(want["reasoningLevel"], "low");
                for (message, want) in call
                    .messages
                    .iter()
                    .zip(want["messages"].as_array().unwrap())
                {
                    assert_eq!(message["role"], want["role"], "{name} call {index} role");
                    assert_text(
                        message["content"].as_str().unwrap(),
                        &want["content"],
                        &format!("{name} call {index}"),
                    );
                }
            }
            output
        };
        let expected = &case["output"];
        for (key, value) in expected.as_object().unwrap() {
            if key == "content" {
                assert_text(
                    output[key].as_str().unwrap(),
                    value,
                    &format!("{name} content"),
                );
            } else {
                assert_eq!(&output[key], value, "{name} output.{key}");
            }
        }
        assert_eq!(
            output.as_object().unwrap().len(),
            expected.as_object().unwrap().len(),
            "{name} output keys {:?}",
            output.as_object().unwrap().keys().collect::<Vec<_>>()
        );
        assert_text(
            &model_content(&output),
            &case["modelContent"],
            &format!("{name} model content"),
        );
    }
}

#[test]
fn stringify_keeps_key_order_and_js_formatting() {
    use crate::session_context_text::{iso_time, stringify_in_order};
    assert_eq!(
        stringify_in_order(r#"{ "z": 1, "a": [true, null, "é\n"], "z": 2 }"#).as_deref(),
        Some(r#"{"z":2,"a":[true,null,"é\n"]}"#)
    );
    assert_eq!(iso_time(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso_time(1_767_323_045_123), "2026-01-02T03:04:05.123Z");
}
