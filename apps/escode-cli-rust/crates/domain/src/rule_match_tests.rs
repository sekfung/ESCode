//! `RuleMatcher` 与 JS `new RegExp` 的语义对照。

use super::rule_match::RuleMatcher;

fn matches(pattern: &str, ignore_case: bool, text: &str) -> bool {
    RuleMatcher::new(pattern, ignore_case)
        .unwrap_or_else(|error| panic!("{pattern}: {error}"))
        .is_match(text)
}

#[test]
fn plain_patterns_keep_rust_regex_semantics() {
    assert!(matches(".*glm-5\\..*", true, "GLM-5.1"));
    assert!(!matches("glm-5\\.1", true, "GLM-5.1-extra"));
    assert!(matches(
        "https://api\\.example\\.com/v1",
        false,
        "https://api.example.com/v1"
    ));
    assert!(!matches(
        "https://api\\.example\\.com/v1",
        false,
        "https://API.example.com/v1"
    ));
}

#[test]
fn negative_lookahead_excludes_vision_models() {
    // 历史 Built-in 的 GLM-4.6 Messages 规则：`(?!v)` 把视觉型号排除在同一条规则外。
    let pattern = ".*glm-4\\.6(?!v).*";
    assert!(matches(pattern, true, "glm-4.6"));
    assert!(matches(pattern, true, "GLM-4.6-flash"));
    assert!(!matches(pattern, true, "glm-4.6v"));
    assert!(!matches(pattern, true, "glm-4.6v-flash"));
}

#[test]
fn lookbehind_and_backreferences_follow_javascript() {
    // TS 把规则包成 `^(?:pattern)$`：后顾在偏移 0 处无法成功，所以后顾前必须能吃掉前缀。
    assert!(!matches("(?<=glm-)4\\.6", false, "glm-4.6"));
    assert!(matches(".*(?<=glm-)4\\.6", false, "glm-4.6"));
    assert!(!matches(".*(?<=openai-)4\\.6", false, "glm-4.6"));
    assert!(matches("(glm)-\\1", false, "glm-glm"));
    assert!(!matches("(glm)-\\1", false, "glm-openai"));
}

#[test]
fn invalid_patterns_still_fail_to_compile() {
    assert!(RuleMatcher::new("(", true).is_err());
}
