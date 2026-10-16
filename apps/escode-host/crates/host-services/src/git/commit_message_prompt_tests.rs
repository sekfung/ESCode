use serde_json::json;

use super::*;

#[test]
fn conventional_subjects() {
    for good in [
        "feat: add x",
        "fix(core): y",
        "refactor(a-b)!: z",
        "revert: w",
        "chore!: q",
    ] {
        assert!(is_conventional_subject(good), "{good}");
    }
    for bad in [
        "feature: x",
        "feat:x",
        "feat(): x",
        "feat(a)b: x",
        "Feat: x",
        "feat: ",
        "just text",
    ] {
        assert!(!is_conventional_subject(bad), "{bad}");
    }
    assert!(is_conventional_subject(&format!(
        "feat: {}",
        "a".repeat(100)
    )));
    assert!(!is_conventional_subject(&format!(
        "feat: {}",
        "a".repeat(101)
    )));
    assert!(
        !is_conventional_subject(&format!("feat: {}", "😀".repeat(51))),
        "astral 字符按 2 个码元计"
    );
}

#[test]
fn validate_strips_fences_quotes_and_prefixes() {
    for raw in [
        "```\nfeat: a\n```",
        "```text\nfeat: a\n```",
        "\"feat: a\"",
        "'feat: a'",
        "Commit message: feat: a",
        "  feat: a  ",
    ] {
        match validate(raw) {
            Validation::Ok(message) => assert_eq!(message, "feat: a", "{raw}"),
            Validation::Invalid { .. } => panic!("{raw} should validate"),
        }
    }
    assert!(matches!(
        validate("   "),
        Validation::Invalid { preview: None }
    ));
    let Validation::Invalid { preview } = validate("Here is the message\nfeat: a") else {
        panic!("invalid")
    };
    assert_eq!(preview.as_deref(), Some("Here is the message"));
}

#[test]
fn prompt_uses_fallbacks_and_truncation_markers() {
    let prompt = build_prompt(None, None, Some("zh_CN"), &[], &[], &json!(null));
    assert!(prompt.contains("Current branch: (detached or unknown)"));
    assert!(prompt.contains("Current language: Chinese"));
    assert!(prompt.contains("- (no file summary available)"));
    assert!(prompt.contains("(diff unavailable; infer only from the changed file summary)"));
    let files: Vec<_> = (0..25).map(|i| json!({"kind": "modified", "repoRelativePath": format!("f{i}"), "added": 1, "removed": 2})).collect();
    let prompt = build_prompt(Some("main"), Some("en-US"), None, &files, &[], &json!(null));
    assert!(prompt.contains("- ... 5 more files") && prompt.contains("Current language: English"));
}

/// TS 真实生成器导出的黄金样本（`scripts/generate-escode-host-commit-message-fixtures.mjs`）。
#[test]
fn prompts_and_validation_match_the_ts_generator_byte_for_byte() {
    let golden: Value =
        serde_json::from_str(include_str!("commit_message_golden.json")).expect("golden");
    assert_eq!(golden["fixtureVersion"], 1);
    for case in golden["prompts"].as_array().expect("prompts") {
        let files: Vec<Value> = case["files"].as_array().cloned().unwrap_or_default();
        let diffs: Vec<Value> = case["diffs"].as_array().cloned().unwrap_or_default();
        let prompt = build_prompt(
            case["branchName"].as_str(),
            case["locale"].as_str(),
            None,
            &files,
            &diffs,
            case.get("conversationContext").unwrap_or(&Value::Null),
        );
        assert_eq!(
            prompt,
            case["prompt"].as_str().expect("prompt"),
            "prompt case {}",
            case["name"]
        );
    }
    for case in golden["validations"].as_array().expect("validations") {
        let text = case["text"].as_str().expect("text");
        match finalize_model_text(text) {
            Ok(message) => {
                assert_eq!(case["ok"], true, "{text:?}");
                assert_eq!(
                    message,
                    case["message"].as_str().expect("message"),
                    "{text:?}"
                );
            }
            Err(failure) => {
                assert_eq!(case["ok"], false, "{text:?}");
                assert_eq!(
                    failure.message,
                    case["message"].as_str().expect("message"),
                    "{text:?}"
                );
                assert_eq!(
                    failure.reason,
                    case["reason"].as_str().expect("reason"),
                    "{text:?}"
                );
                let expected = case["detail"].as_str();
                assert_eq!(failure.detail.as_deref(), expected, "{text:?}");
            }
        }
    }
}
