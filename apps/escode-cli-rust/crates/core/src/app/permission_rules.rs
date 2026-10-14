//! 权限确认的选项与规则投影（从 permission_flow.rs 拆出）。

use crate::domain::permission::Rule;
use crate::domain::permission_options::{default_updates, options};
use serde_json::{Value, json};

/// 形状与 TS product-projection 一致；response 是旧客户端的兼容兜底，真正语义由 optionId 决定。
pub(super) fn full_access_option() -> Value {
    json!({
        "optionId": FULL_ACCESS_OPTION_ID,
        "label": "Full access",
        "kind": "custom",
        "response": {"decision": "deny", "reason": "Full access requires V4 approval"},
    })
}

pub(super) const FULL_ACCESS_OPTION_ID: &str = "fullAccess";

/// TS `askOptions.allowAlways: false`（no-always-allow）：每次调用都是不同代码，持久或会话规则
/// 不是「记住这次决定」而是把这道确认永久关掉，所以只给「允许一次 / 拒绝」。
const NO_ALWAYS_ALLOW: [&str; 1] = ["SaveWorkflow"];
/// TS `askOptions.allowAlways: "session"`：只给会话作用域的免确认（不落项目规则）。
const SESSION_ALWAYS_ALLOW: [&str; 3] = ["CreateWorkflow", "AmendWorkflow", "FillWorkflowHole"];
pub(super) const OPTIONS_POLICY_TOOLS: [&str; 4] = [
    "CreateWorkflow",
    "AmendWorkflow",
    "SaveWorkflow",
    "FillWorkflowHole",
];

pub(super) fn options_for(tool: &str, suggested: &[Rule], persistent: bool) -> Vec<Value> {
    let mut list = if SESSION_ALWAYS_ALLOW.contains(&tool) {
        options(suggested, true)
    } else {
        options_with_persistence(suggested, persistent)
    };
    if NO_ALWAYS_ALLOW.contains(&tool) {
        list.retain(|option| option["kind"] != "allowAlways");
    }
    // TS product-projection：Create / Amend 的确认另带 workflow Refine（静态应答是普通拒绝，带反馈时
    // 由交互层升级成修改意见）。
    if matches!(tool, "CreateWorkflow" | "AmendWorkflow") {
        list.push(json!({
            "optionId": "workflowRefine",
            "label": "Refine",
            "kind": "custom",
            "response": {"decision": "deny", "reason": "Denied"},
        }));
    }
    list
}

fn options_with_persistence(suggested: &[Rule], persistent: bool) -> Vec<Value> {
    let mut list = options(suggested, !persistent);
    if persistent {
        return list;
    }
    // 无法持久化项目规则时不投放 allowAlways；会话授权的语义见 spec。
    list.retain(|o| o["optionId"] != "allowAlways");
    list
}

/// 「总是允许」的规则：Bash 用稳定前缀建议，其余工具按输入键取内容。
pub(super) fn suggested_rules(tool: &str, input: &Value) -> Vec<Rule> {
    if tool == "Bash"
        && let Some(command) = input["command"].as_str()
    {
        let rules: Vec<Rule> = crate::domain::bash_rule_policy::BashRulePolicy::new(command)
            .suggestions
            .into_iter()
            .map(|content| Rule {
                tool_name: tool.to_owned(),
                rule_content: Some(content),
            })
            .collect();
        if !rules.is_empty() {
            return rules;
        }
    }
    default_updates(tool, input)
}

/// 把建议规则并入一份 ruleset JSON 的 allow 段（去重）。
pub(super) fn merge_allow_rules(json: &mut Value, rules: &[Rule]) {
    let mut merged = json["allow"].as_array().cloned().unwrap_or_default();
    for rule in rules {
        let entry = match &rule.rule_content {
            Some(content) => json!({"toolName": rule.tool_name, "ruleContent": content}),
            None => json!({"toolName": rule.tool_name}),
        };
        let duplicate = merged.iter().any(|existing| {
            existing["toolName"] == entry["toolName"]
                && existing["ruleContent"] == entry["ruleContent"]
        });
        if !duplicate {
            merged.push(entry);
        }
    }
    json["allow"] = Value::Array(merged);
}
