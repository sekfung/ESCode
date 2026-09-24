//! 权限规则匹配：逐位对应 TS `permission/service.ts` 的 matchesProjectRules / matchesRule /
//! ruleSubjects / matchesRuleContent 与 `rule-matching.ts`、`webfetch-preapproved.ts`。
//! Bash 的 rulePolicy（复合命令拆分）不在此处，随 Bash 只读分类一起移植。
use serde_json::Value;

pub const OFFICIAL_CUA_RULE_TOOL_NAME: &str = "zcode:permission-capability:official_cua";
pub const OFFICIAL_CUA_GROUP: &str = "official_cua";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub tool_name: String,
    pub rule_content: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Ruleset {
    pub allow: Vec<Rule>,
    pub ask: Vec<Rule>,
    pub deny: Vec<Rule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleBehavior {
    Allow,
    Ask,
    Deny,
}

impl Ruleset {
    /// 解析 TS `PermissionRuleset`（`{version, allow?, ask?, deny?}`，条目为 `{toolName, ruleContent?}`）；
    /// 非数组或缺字段的条目忽略，与 TS `Array.isArray` 守卫一致。
    pub fn from_json(value: &Value) -> Self {
        let rules = |key: &str| {
            value[key]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|r| {
                            Some(Rule {
                                tool_name: r["toolName"].as_str()?.to_owned(),
                                rule_content: r["ruleContent"].as_str().map(str::to_owned),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            allow: rules("allow"),
            ask: rules("ask"),
            deny: rules("deny"),
        }
    }

    fn rules(&self, behavior: RuleBehavior) -> &[Rule] {
        match behavior {
            RuleBehavior::Allow => &self.allow,
            RuleBehavior::Ask => &self.ask,
            RuleBehavior::Deny => &self.deny,
        }
    }
}

pub(crate) fn matches(
    ruleset: Option<&Ruleset>,
    behavior: RuleBehavior,
    tool_name: &str,
    input: &Value,
    group: Option<&str>,
) -> bool {
    let Some(ruleset) = ruleset else {
        return false;
    };
    ruleset
        .rules(behavior)
        .iter()
        .filter(|rule| in_scope(rule, tool_name, group))
        .any(|rule| match &rule.rule_content {
            // TS `if (!rule.ruleContent) return true`：空串同样视为无内容限制。
            None => true,
            Some(content) if content.is_empty() => true,
            Some(content) => subjects(input, tool_name)
                .iter()
                .any(|subject| content_matches(subject, content)),
        })
}

fn in_scope(rule: &Rule, tool_name: &str, group: Option<&str>) -> bool {
    if rule.tool_name == OFFICIAL_CUA_RULE_TOOL_NAME {
        return group == Some(OFFICIAL_CUA_GROUP);
    }
    rule.tool_name == tool_name || (tool_name == "Write" && rule.tool_name == "Edit")
}

fn subjects(input: &Value, tool_name: &str) -> Vec<String> {
    if let Some(text) = input.as_str() {
        return vec![text.to_owned()];
    }
    let Some(record) = input.as_object() else {
        return vec![];
    };
    if tool_name == "WebFetch"
        && let Some(url) = record.get("url").and_then(Value::as_str)
    {
        return domain_subject(url).into_iter().collect();
    }
    for key in [
        "command",
        "url",
        "file_path",
        "path",
        "pattern",
        "patch_text",
    ] {
        if let Some(value) = record.get(key).and_then(Value::as_str) {
            return vec![value.to_owned()];
        }
    }
    vec![]
}

fn domain_subject(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    (!host.is_empty()).then(|| format!("domain:{host}"))
}

pub(crate) fn content_matches(subject: &str, content: &str) -> bool {
    if let Some(prefix) = content.strip_suffix(":*") {
        return subject == prefix
            || subject
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with([' ', '\t']));
    }
    if content.contains('*') {
        return wildcard(subject, content);
    }
    subject == content
}

/// TS `wildcardToRegExp`：`*` 匹配任意字符（`.*`，JS 默认不跨换行），其余字面量，整串锚定。
fn wildcard(subject: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !subject.starts_with(first) || subject.len() < first.len() + last.len() {
        return false;
    }
    let body = &subject[first.len()..subject.len() - last.len()];
    if !subject.ends_with(last) || body.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return false;
    }
    let mut rest = body;
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

pub(crate) fn webfetch_preapproved(tool_name: &str, input: &Value) -> bool {
    if tool_name != "WebFetch" {
        return false;
    }
    let Some(url) = input.get("url").and_then(Value::as_str) else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    static LISTS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let lists = LISTS.get_or_init(|| {
        serde_json::from_str(include_str!("webfetch_preapproved.json"))
            .expect("generated webfetch list")
    });
    if lists["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h.as_str() == Some(host))
    {
        return true;
    }
    let Some(prefixes) = lists["pathPrefixes"][host].as_array() else {
        return false;
    };
    let path = parsed.path();
    let lower = path.to_ascii_lowercase();
    // TS：/%(25)*(2f|5c|2e)/i —— 编码的分隔符或点（含多重编码）一律不放行。
    let encoded = lower.match_indices('%').any(|(i, _)| {
        let rest = lower[i + 1..].trim_start_matches("25");
        ["2f", "5c", "2e"].iter().any(|s| rest.starts_with(s))
    });
    if encoded {
        return false;
    }
    prefixes.iter().filter_map(Value::as_str).any(|prefix| {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|r| r.starts_with('/'))
    })
}
