//! 规则编译：JS 方言与“非法写法仍报错”的边界。

use super::rules;
use serde_json::json;

#[test]
fn lookahead_rules_do_not_fail_the_registry() {
    // CLI 侧此前用 `regex` crate：带前瞻的规则让 `rules()` 返回 Err，整份 Registry 解析失败；
    // TS 用 `new RegExp` 能正常命中，这里钉住同一结果。
    let config = json!({"modelRules":[
        {"modelMatch":".*glm-4\\.6(?!v).*","config":{"enabled":true}},
    ]});
    let compiled = rules(&config, false).expect("lookahead rule must compile");
    let matcher = compiled[0].model.as_ref().expect("modelMatch");
    assert!(matcher.is_match("GLM-4.6-flash"));
    assert!(!matcher.is_match("glm-4.6v"));
}

#[test]
fn invalid_patterns_still_fail_the_registry() {
    let config = json!({"modelRules":[{"modelMatch":"(","config":{}}]});
    assert!(rules(&config, false).is_err());
}
