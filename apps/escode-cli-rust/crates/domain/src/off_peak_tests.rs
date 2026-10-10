//! 语料由 scripts/generate-escode-cli-rust-offpeak-corpus.mjs 以 TS handler + 协议端口为 oracle 生成。

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use serde_json::Value;

use super::*;

const CORPUS: &str = include_str!("../tests/fixtures/offpeak_corpus.json");

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("scripted host future must be ready"),
    }
}

#[test]
fn flows_match_ts() {
    let corpus: Value = serde_json::from_str(CORPUS).unwrap();
    // Host 应答需要保持键顺序（任务快照进入模型文案）。
    let ordered = Json::parse(CORPUS).unwrap();
    let ordered_cases = ordered.get("cases").and_then(Json::as_array).unwrap();
    for (case, ordered_case) in corpus["cases"].as_array().unwrap().iter().zip(ordered_cases) {
        let name = case["name"].as_str().unwrap();
        let turn = Turn {
            off_peak_turn: case["offPeakTurn"] == true,
            active_off_peak_task_id: case["activeOffPeakTaskId"].as_str().map(str::to_owned),
            session_id: "sess_current".into(),
        };
        let host = ordered_case.get("host").unwrap();
        let mut used: std::collections::BTreeMap<String, usize> = Default::default();
        let mut requests: Vec<Value> = Vec::new();
        let outcome = block_on(run(case["tool"].as_str().unwrap(), &case["input"], &turn, |method, params| {
            requests.push(serde_json::json!({"method": method, "params": params}));
            let index = used.entry(method.to_owned()).or_default();
            let step = host.get(method).and_then(Json::as_array).and_then(|s| s.get(*index)).cloned();
            *index += 1;
            let reply = match step {
                Some(step) => match step.get("error") {
                    Some(error) => Err(HostError {
                        code: match error.get("code") {
                            Some(Json::Number(n)) => n.as_i64().unwrap(),
                            _ => 0,
                        },
                        message: error.get("message").and_then(Json::as_str).unwrap().into(),
                    }),
                    None => Ok(step.get("result").cloned().unwrap()),
                },
                None => Err(HostError { code: 0, message: format!("unscripted {method}") }),
            };
            std::future::ready(reply)
        }));
        assert_eq!(Value::Array(requests), case["requests"], "{name} requests");
        match outcome {
            Ok((output, content)) => {
                assert_eq!(output, case["output"], "{name} output");
                assert_eq!(content, case["modelContent"].as_str().unwrap(), "{name} model content");
            }
            Err(error) => assert_eq!(error, case["error"].as_str().unwrap(), "{name} error"),
        }
    }
}
