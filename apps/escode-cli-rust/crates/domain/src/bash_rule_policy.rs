//! Bash 权限规则策略：对应 TS `bash-command-permission-policy.ts` 与 `bash-command-rule-evaluator.ts`。
//! 复合命令按调用拆分匹配规则；allow 只要求非只读调用全部命中，deny/ask 任一调用命中即成立。
use crate::bash_parse::{Analysis, Invocation, analyze};
use crate::bash_rule_prefix::{assignments, stable_prefix};
use crate::permission_rules::Rule;

const MAX_SUGGESTED_RULES: usize = 5;

pub struct BashRulePolicy {
    exact: Vec<String>,
    safe: bool,
    all: Vec<Vec<String>>,
    required: Vec<Vec<String>>,
    /// 「总是允许」建议的 ruleContent 列表（TS suggestedPermissionUpdates 的规则）。
    pub suggestions: Vec<String>,
}

impl BashRulePolicy {
    pub fn new(command: &str) -> Self {
        let raw = command.trim().to_owned();
        let exact = if command == raw {
            vec![raw.clone()]
        } else {
            vec![command.to_owned(), raw.clone()]
        };
        let analysis = analyze(command);
        let safe = safe_for_prefix(&analysis);
        let all = if safe {
            analysis.commands.iter().map(subjects).collect()
        } else {
            vec![]
        };
        let required: Vec<&Invocation> = if safe {
            analysis
                .commands
                .iter()
                .filter(|i| !crate::bash_policy::is_readonly(&i.command_text))
                .collect()
        } else {
            vec![]
        };
        let suggestions = suggest(&raw, safe, &required);
        Self {
            exact,
            safe,
            all,
            required: required.into_iter().map(subjects).collect(),
            suggestions,
        }
    }

    /// TS `evaluateBashRules`：rules 已按工具作用域过滤。
    pub fn evaluate(&self, allow: bool, rules: &[&Rule]) -> bool {
        let content = |r: &Rule| r.rule_content.clone().filter(|c| !c.is_empty());
        if rules.iter().any(|r| content(r).is_none()) {
            return true;
        }
        if self.exact.iter().any(|c| !c.is_empty())
            && rules
                .iter()
                .any(|r| self.exact.contains(r.rule_content.as_ref().unwrap()))
        {
            return true;
        }
        if !self.safe {
            return false;
        }
        let groups = if allow { &self.required } else { &self.all };
        if groups.is_empty() {
            return false;
        }
        let hit = |subjects: &Vec<String>| {
            subjects.iter().any(|s| {
                rules.iter().any(|r| {
                    crate::permission_rules::content_matches(s, r.rule_content.as_ref().unwrap())
                })
            })
        };
        if allow {
            groups.iter().all(hit)
        } else {
            groups.iter().any(hit)
        }
    }
}

fn safe_for_prefix(a: &Analysis) -> bool {
    a.permission_safe()
        && !a.has_redirects
        && !a.commands.is_empty()
        && a.commands
            .iter()
            .all(|i| i.redirects.is_empty() && !i.has_dynamic_words && assignments(i).is_some())
}

fn subjects(i: &Invocation) -> Vec<String> {
    let mut raw = assignments(i).unwrap_or_default();
    raw.extend(i.argv.iter().cloned());
    let raw = raw.join(" ");
    match stable_prefix(i) {
        Some(p) if p != raw => vec![raw, p],
        _ => vec![raw],
    }
}

fn suggest(raw: &str, safe: bool, required: &[&Invocation]) -> Vec<String> {
    if raw.is_empty() {
        return vec![];
    }
    let exact = || vec![raw.to_owned()];
    if !safe || required.is_empty() || required.len() > MAX_SUGGESTED_RULES {
        return exact();
    }
    let mut rules: Vec<String> = vec![];
    for i in required {
        let Some(prefix) = stable_prefix(i) else {
            return exact();
        };
        let rule = format!("{prefix}:*");
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    if rules.is_empty() || rules.len() > MAX_SUGGESTED_RULES {
        exact()
    } else {
        rules
    }
}
