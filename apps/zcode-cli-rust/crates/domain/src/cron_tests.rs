//! 语料由 scripts/generate-zcode-cli-rust-cron-corpus.mjs 以 TS handler + 协议端口为 oracle 生成。

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use serde_json::Value;

use super::*;

const CORPUS: &str = include_str!("../tests/fixtures/cron_corpus.json");

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("scripted host future must be ready"),
    }
}

#[test]
fn validation_matches_ts() {
    let corpus: Value = serde_json::from_str(CORPUS).unwrap();
    for case in corpus["validationCases"].as_array().unwrap() {
        let tool = case["tool"].as_str().unwrap();
        let parsed = parse(tool, &case["input"]);
        assert_eq!(
            parsed.is_ok(),
            case["ok"] == true,
            "{tool} {}: {parsed:?}",
            case["input"]
        );
        if let Ok(data) = parsed {
            assert_eq!(
                Value::Object(data),
                case["data"],
                "{tool} {}",
                case["input"]
            );
        }
    }
}

#[test]
fn flows_match_ts() {
    let corpus: Value = serde_json::from_str(CORPUS).unwrap();
    // Host 应答需要保持键顺序（嵌套 scheduleRule 进入模型文案）。
    let ordered = Json::parse(CORPUS).unwrap();
    let ordered_flows = ordered.get("flowCases").and_then(Json::as_array).unwrap();
    for (case, ordered_case) in corpus["flowCases"]
        .as_array()
        .unwrap()
        .iter()
        .zip(ordered_flows)
    {
        let name = case["name"].as_str().unwrap();
        let turn = Turn {
            automation_turn: case["automationTurn"] == true,
            active_automation_id: case["activeAutomationId"].as_str().map(str::to_owned),
            bot_delivery_target: Some(case["bot"].clone()).filter(|b| !b.is_null()),
            session_id: "sess_current".into(),
            mode: case["mode"].as_str().unwrap().into(),
            model_selection: Some(case["selection"].clone()),
        };
        let host = ordered_case.get("host").unwrap();
        let mut used: std::collections::BTreeMap<String, usize> = Default::default();
        let mut requests: Vec<Value> = Vec::new();
        let outcome = block_on(run(
            case["tool"].as_str().unwrap(),
            &case["input"],
            &turn,
            |method, params| {
                requests.push(serde_json::json!({"method": method, "params": params}));
                let index = used.entry(method.to_owned()).or_default();
                let step = host
                    .get(method)
                    .and_then(Json::as_array)
                    .and_then(|s| s.get(*index))
                    .cloned();
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
                    None => Err(HostError {
                        code: 0,
                        message: format!("unscripted {method}"),
                    }),
                };
                std::future::ready(reply)
            },
        ));
        assert_eq!(Value::Array(requests), case["requests"], "{name} requests");
        let titles: Vec<Value> = outcome
            .freeze_title
            .iter()
            .map(|t| Value::from(t.as_str()))
            .collect();
        assert_eq!(Value::Array(titles), case["titles"], "{name} titles");
        match &outcome.output {
            Some((output, content)) => {
                assert_eq!(output, &case["output"], "{name} output");
                assert_eq!(
                    content,
                    case["modelContent"].as_str().unwrap(),
                    "{name} model content"
                );
            }
            None => {
                assert_eq!(
                    outcome.error.as_deref(),
                    case["error"].as_str(),
                    "{name} error"
                );
                assert_eq!(outcome.limit, case["limit"] == true, "{name} limit");
            }
        }
    }
}

#[test]
fn turn_facts_match_ts() {
    assert_eq!(turn_automation_id(Some(" a1 "), "x").as_deref(), Some("a1"));
    assert_eq!(
        turn_automation_id(None, "automation-abc:run-1").as_deref(),
        Some("automation-abc")
    );
    assert_eq!(turn_automation_id(None, "automation-"), None);
    assert_eq!(turn_automation_id(None, "cmd-1"), None);
    let tools = turn_disallowlist(&["Bash".into()], Some("a"));
    assert_eq!(tools, ["Bash", "CronCreate", "CronUpdate", "CronDelete"]);
    assert!(is_automation_turn(None, &tools));
    assert!(!is_automation_turn(None, &["CronCreate".into()]));
}
