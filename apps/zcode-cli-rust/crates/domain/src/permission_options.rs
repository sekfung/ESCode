//! 权限交互的选项与拒绝文案：对应 TS `bootstrap/permission-options.ts` 与 v4 投影
//! （`product-projection.ts` 的 kind/label/optionId 映射）。optionId 是应答侧精确命中的键，
//! 必须与投放值一致。
use crate::permission_rules::Rule;
use serde_json::{Value, json};

/// 用户拒绝时回给模型的内容，与 TS 逐字一致（含「工具未执行」的明确告知）。
pub const DENIED_CONTENT: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";

pub fn denied_content(feedback: Option<&str>) -> String {
    match feedback.map(str::trim).filter(|f| !f.is_empty()) {
        Some(feedback) => {
            format!("{DENIED_CONTENT} To tell you how to proceed, the user said:\n{feedback}")
        }
        None => DENIED_CONTENT.to_owned(),
    }
}

/// TS `ruleContentFromInput`：按固定键序取第一个字符串作为规则内容。
const INPUT_KEYS: [&str; 6] = [
    "command",
    "url",
    "file_path",
    "path",
    "pattern",
    "patch_text",
];

pub fn default_updates(tool_name: &str, input: &Value) -> Vec<Rule> {
    let content = INPUT_KEYS
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::to_owned);
    vec![Rule {
        tool_name: tool_name.to_owned(),
        rule_content: content,
    }]
}

fn update_json(rules: &[Rule]) -> Value {
    json!({
        "behavior": "allow",
        "type": "addRules",
        "rules": rules
            .iter()
            .map(|r| match &r.rule_content {
                Some(c) => json!({"toolName": r.tool_name, "ruleContent": c}),
                None => json!({"toolName": r.tool_name}),
            })
            .collect::<Vec<_>>(),
    })
}

/// v4 投影后的选项列表：allowOnce / allowAlways（项目或会话）/ deny。
/// `suggested` 为「总是允许」要写入的规则；`session_scope` 表示按会话授权（无项目规则持久化）。
pub fn options(suggested: &[Rule], session_scope: bool) -> Vec<Value> {
    let remember = if session_scope {
        json!({
            "optionId": "allowSession",
            "label": "Always allow in this session",
            "kind": "allowAlways",
            "response": {"decision": "allow", "reason": "Approved for this session"},
        })
    } else {
        json!({
            "optionId": "allowAlways",
            "label": "Always allow in this project",
            "kind": "allowAlways",
            "response": {
                "decision": "allow",
                "permissionUpdates": [update_json(suggested)],
                "reason": "Approved for this project",
            },
        })
    };
    vec![
        json!({
            "optionId": "allowOnce",
            "label": "Allow once",
            "kind": "allowOnce",
            "response": {"decision": "allow", "reason": "Approved once"},
        }),
        remember,
        json!({
            "optionId": "deny",
            "label": "Deny",
            "kind": "deny",
            "response": {"decision": "deny", "reason": DENIED_CONTENT},
        }),
    ]
}
