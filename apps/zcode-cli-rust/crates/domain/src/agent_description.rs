//! Agent 工具的 provider 描述，按 TS `buildAgentProviderDescription` + `formatAgentProfilesForPrompt` 渲染。
//! 头尾文案来自生成资产（dynamicWorkflowEnabled=false：Rust 不支持工作流）。见 docs/specs/rust-tool-surface.md。

use std::sync::OnceLock;

use serde_json::Value;

use crate::subagent::Profile;

/// 子代理工具面强制剔除 plan 工具（TS `SUBAGENT_CHILD_FORCED_DISALLOWED_TOOLS`）。
const CHILD_FORCED_DISALLOWED: [&str; 2] = ["EnterPlanMode", "ExitPlanMode"];

fn template() -> &'static Value {
    static TEMPLATE: OnceLock<Value> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        serde_json::from_str(include_str!("agent_description_template.json"))
            .expect("generated agent description template")
    })
}

pub fn render(profiles: &[Profile], embedded_search: bool) -> String {
    let template = template();
    let frame = &template["frame"];
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let list = if profiles.is_empty() {
        // TS：没有 profile 时列表为 null，join 后为空行。
        String::new()
    } else {
        let explore = &template["exploreTools"][if embedded_search {
            "embedded"
        } else {
            "direct"
        }];
        std::iter::once(text(&frame["listHeader"]))
            .chain(profiles.iter().map(|profile| {
                let tools = if profile.name == "Explore" && profile.source == "built-in" {
                    Some(text(explore))
                } else {
                    profile
                        .tools
                        .as_ref()
                        .map(|tools| child_tools(tools, &profile.disallowed_tools).join(", "))
                };
                match tools.filter(|tools| !tools.is_empty()) {
                    Some(tools) => {
                        format!(
                            "- {}: {} (Tools: {tools})",
                            profile.name, profile.description
                        )
                    }
                    None => format!("- {}: {}", profile.name, profile.description),
                }
            }))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!("{}{list}{}", text(&frame["head"]), text(&frame["tail"]))
}

/// TS `filterSubagentChildToolNames`：规则名取 `(` 之前，`web_search` 视为 WebSearch。
fn child_tools<'a>(tools: &'a [String], disallowed: &[String]) -> Vec<&'a str> {
    let rule_name = |rule: &str| {
        let trimmed = rule.trim();
        let name = match trimmed.find('(') {
            Some(index) if index > 0 => &trimmed[..index],
            _ => trimmed,
        };
        alias(name).to_owned()
    };
    let blocked: Vec<String> = CHILD_FORCED_DISALLOWED
        .iter()
        .map(|name| (*name).to_owned())
        .chain(disallowed.iter().map(|rule| rule_name(rule)))
        .filter(|name| !name.is_empty())
        .collect();
    tools
        .iter()
        .map(String::as_str)
        .filter(|tool| !blocked.iter().any(|name| name == alias(tool)))
        .collect()
}

fn alias(name: &str) -> &str {
    if name == "web_search" {
        "WebSearch"
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, source: &str, tools: Option<&[&str]>, disallowed: &[&str]) -> Profile {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "description": format!("{name} agent"),
            "source": source,
            "systemPrompt": "",
            "tools": tools,
            "disallowedTools": disallowed,
        }))
        .unwrap()
    }

    #[test]
    fn renders_profiles_like_ts() {
        let text = render(
            &[
                profile("general-purpose", "built-in", Some(&["*"]), &[]),
                profile("Explore", "built-in", Some(&["Bash", "Glob"]), &[]),
                profile(
                    "custom",
                    "user",
                    Some(&["Read", "Bash", "EnterPlanMode", "web_search"]),
                    &["Bash(rm:*)", "WebSearch"],
                ),
                profile("bare", "project", None, &[]),
            ],
            true,
        );
        assert!(text.contains("- general-purpose: general-purpose agent (Tools: *)\n"));
        assert!(text.contains(
            "- Explore: Explore agent (Tools: Read, Bash, WebFetch, WebSearch, TodoWrite)\n"
        ));
        assert!(text.contains("- custom: custom agent (Tools: Read)\n"));
        assert!(text.contains("- bare: bare agent\n"));
        assert!(!text.contains("Current profile catalog"));
    }
}
