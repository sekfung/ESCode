//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-custom-commands.mjs）。
use super::custom_commands::{default_roots, discover_in};
use crate::domain::custom_command::{self as cc, Segment};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../tests/fixtures/custom_commands_corpus.json"
    ))
    .unwrap()
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

#[test]
fn templates_and_arguments_match_ts() {
    let corpus = corpus();
    for case in corpus["templates"].as_array().unwrap() {
        let expansion = cc::expand_template(
            case["content"].as_str().unwrap(),
            case["args"].as_str().unwrap(),
        );
        assert_eq!(expansion.body, case["body"], "{case}");
        assert_eq!(expansion.argument_count, case["argumentCount"], "{case}");
        let prompt = cc::format_prompt(
            "review",
            case["scope"].as_str().unwrap_or("project"),
            case["source"].as_str().unwrap_or("zcode"),
            &strings(&case["skills"]),
            &expansion.body,
        );
        assert_eq!(prompt, case["prompt"], "{case}");
    }
    for case in corpus["splits"].as_array().unwrap() {
        assert_eq!(
            cc::split_arguments(case["input"].as_str().unwrap()),
            strings(&case["args"]),
            "{case}"
        );
    }
}

/// 以语料中的假执行器（stdout 为 `[cmd]\n\n`，fail 用例退出码 2）重放切分、守卫、环境与失败文案。
#[test]
fn shell_expansion_matches_ts() {
    for case in corpus()["shells"].as_array().unwrap() {
        let session = case["sessionId"].as_str();
        let plugin = cc::infer_plugin(
            case["source"].as_str().unwrap_or("zcode"),
            case["rootPath"].as_str().unwrap_or("/x"),
        );
        let mut runs = vec![];
        let mut output = String::new();
        let mut error = None;
        for segment in cc::shell_segments(case["content"].as_str().unwrap()) {
            match segment {
                Segment::Text(text) => output.push_str(&text),
                Segment::Shell(shell) if shell.is_empty() => {}
                Segment::Shell(shell) => {
                    if let Err(e) = cc::check_shell_context(
                        "review",
                        &shell,
                        session.is_some(),
                        plugin.is_some(),
                    ) {
                        error = Some(e);
                        break;
                    }
                    let env: serde_json::Map<String, Value> =
                        cc::shell_env("/work", session, plugin.as_ref())
                            .into_iter()
                            .map(|(k, v)| (k, v.into()))
                            .collect();
                    runs.push(json!({"command":shell,"cwd":"/work","env":env}));
                    if case["fail"] == true {
                        error = Some(cc::shell_failure(
                            "review",
                            &shell,
                            "2",
                            None,
                            "  boom \n",
                            "partial",
                            "completed",
                        ));
                        break;
                    }
                    output.push_str(&format!("[{shell}]"));
                }
            }
        }
        assert_eq!(Value::Array(runs), case["runs"], "{case}");
        match error {
            Some(e) => assert_eq!(e, case["error"], "{case}"),
            None => assert_eq!(output, case["output"], "{case}"),
        }
    }
}

#[tokio::test]
async fn discovery_matches_ts_adapter() {
    let corpus = corpus();
    let discovery = &corpus["discovery"];
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in discovery["tree"].as_object().unwrap() {
        let target = dir.path().join(path);
        tokio::fs::create_dir_all(target.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&target, content.as_str().unwrap())
            .await
            .unwrap();
    }
    let at = |p: &str| dir.path().join(p);
    let roots = default_roots(
        &at(discovery["home"].as_str().unwrap()),
        &at(discovery["cwd"].as_str().unwrap()),
    )
    .await;
    let disabled: BTreeSet<_> = strings(&discovery["disabled"])
        .iter()
        .map(|p| at(p))
        .collect();
    let found = discover_in(&roots, &disabled, &CancellationToken::new())
        .await
        .unwrap();
    let rel = |p: &std::path::Path| {
        p.strip_prefix(dir.path())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut actual = vec![];
    for command in &found {
        let m = &command.meta;
        let mut entry = json!({
            "allowedTools": m.allowed_tools,
            "description": m.description,
            "disableNonInteractive": m.disable_non_interactive,
            "frontmatterKeys": m.frontmatter_keys,
            "name": m.name,
            "path": rel(&command.path),
            "rootPath": rel(&command.root),
            "scope": m.scope,
            "skills": m.skills,
            "source": m.source,
            "content": super::custom_commands::content(command).await.unwrap(),
        });
        if let Some(hint) = &m.argument_hint {
            entry["argumentHint"] = hint.clone().into();
        }
        if let Some(model) = &m.model {
            entry["model"] = model.clone().into();
        }
        actual.push(entry);
    }
    assert_eq!(Value::Array(actual), discovery["commands"]);
}
