//! 差分：Rust BashRulePolicy 与 TS resolveBashPermissionRulePolicy 的规则判定与「总是允许」建议逐条一致。
use serde_json::Value;
use zcode_cli_domain::bash_rule_policy::BashRulePolicy;
use zcode_cli_domain::permission::Rule;

#[test]
fn rust_bash_rule_policy_matches_ts() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/bash_rule_corpus.json")).unwrap();
    let rulesets: Vec<Vec<Rule>> = fixture["rulesets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|set| {
            set.as_array()
                .unwrap()
                .iter()
                .map(|r| Rule {
                    tool_name: r["toolName"].as_str().unwrap().to_owned(),
                    rule_content: r["ruleContent"].as_str().map(str::to_owned),
                })
                .collect()
        })
        .collect();
    let decisions = fixture["decisions"].as_str().unwrap().as_bytes();
    let mut failures = vec![];
    let mut k = 0;
    for (i, command) in fixture["commands"].as_array().unwrap().iter().enumerate() {
        let command = command.as_str().unwrap();
        let policy = BashRulePolicy::new(command);
        for (r, rules) in rulesets.iter().enumerate() {
            let refs: Vec<&Rule> = rules.iter().collect();
            for allow in [true, false] {
                let want = decisions[k] == b'1';
                k += 1;
                if policy.evaluate(allow, &refs) != want {
                    failures.push(format!(
                        "{command:?} ruleset#{r} allow={allow}: want {want}"
                    ));
                }
            }
        }
        let want: Vec<String> = serde_json::from_value(fixture["suggestions"][i].clone()).unwrap();
        if policy.suggestions != want {
            failures.push(format!(
                "{command:?} suggestions: {:?} != {want:?}",
                policy.suggestions
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures[..failures.len().min(25)].join("\n")
    );
}
