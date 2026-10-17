//! Settings 侧的 agent 摘要解析（TS `subagentMarkdown.ts` 的 `parseSubagentMarkdown`）。
//!
//! 与 runtime 侧（`profile.rs`）**刻意不同**，不要合并：
//! - 输入先用 `yaml_json::parse_runtime_compatible_frontmatter` 做 YAML+宽松合并；
//! - `tools` / `disallowedTools` 只 trim、**不剥** `(...)` 参数（runtime 侧会剥）；
//! - `permissionMode` 对项目级来源**不做**特殊处理（runtime 侧会丢掉）；
//! - `mcpServers` 只要是数组就原样带上（runtime 侧有「必须是父服务器名字列表」的强校验）；
//! - 没有 `memory` 概念（摘要不含该字段）。

use serde_json::{Map, Value, json};

use super::frontmatter::split_markdown_frontmatter;
use super::profile::parse_selection;
use super::yaml_json::parse_runtime_compatible_frontmatter;

const VALID_COLORS: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];
const VALID_PERMISSION_MODES: [&str; 6] = [
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "default",
    "dontAsk",
    "plan",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SummaryScope {
    BuiltIn,
    User,
    Workspace,
}

impl SummaryScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

/// TS `createAgentStateId`。
pub fn agent_state_id(name: &str, scope: SummaryScope, source: &str) -> String {
    format!("{source}:{}:{}", scope.as_str(), name.trim().to_lowercase())
}

/// TS `createPluginAgentStateId`。
pub fn plugin_agent_state_id(plugin_id: &str, agent_name: &str) -> String {
    format!("plugin:{plugin_id}:{}", agent_name.trim().to_lowercase())
}

pub struct SummaryOutcome {
    pub agent: Option<Value>,
    pub diagnostic: Option<Value>,
}

fn diagnostic(code: &str, message: String, path: &str) -> Value {
    json!({"code": code, "message": message, "path": path})
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn normalize_enum(value: Option<&Value>, valid: &[&str]) -> Option<String> {
    scalar_string(value).filter(|raw| valid.contains(&raw.as_str()))
}

fn optional_boolean(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::String(text)) => match text.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn positive_integer(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64().filter(|turns| *turns > 0),
        Some(Value::String(text)) if text.chars().all(|c| c.is_ascii_digit()) => {
            text.parse::<u64>().ok().filter(|turns| *turns > 0)
        }
        _ => None,
    }
}

fn string_list(value: Option<&Value>) -> Option<Vec<String>> {
    let list: Vec<String> = match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::String(text)) => text
            .split([',', ' ', '\t', '\n', '\r'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => return None,
    };
    (!list.is_empty()).then_some(list)
}

/// TS `parseToolSpecList`：数组只 trim（**不剥** `(...)`），字符串按括号深度 0 的逗号/空白切分。
fn tool_spec_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(items)) => {
            let list: Vec<String> = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect();
            (!list.is_empty()).then_some(list)
        }
        Some(Value::String(text)) => {
            let mut result = Vec::new();
            let mut current = String::new();
            let mut depth = 0_i32;
            for character in text.chars() {
                match character {
                    '(' => depth += 1,
                    ')' => depth = (depth - 1).max(0),
                    _ => {}
                }
                if depth == 0 && (character == ',' || character.is_whitespace()) {
                    if !current.trim().is_empty() {
                        result.push(current.trim().to_owned());
                    }
                    current.clear();
                    continue;
                }
                current.push(character);
            }
            if !current.trim().is_empty() {
                result.push(current.trim().to_owned());
            }
            (!result.is_empty()).then_some(result)
        }
        _ => None,
    }
}

fn insert_list(agent: &mut Map<String, Value>, key: &str, list: Option<Vec<String>>) {
    if let Some(list) = list {
        agent.insert(
            key.to_owned(),
            Value::Array(list.into_iter().map(Value::from).collect()),
        );
    }
}

/// TS `parseSubagentMarkdown`。
pub fn parse_subagent_markdown(content: &str, path: &str, scope: SummaryScope) -> SummaryOutcome {
    let split = split_markdown_frontmatter(content);
    let Some(frontmatter) = split.frontmatter else {
        return SummaryOutcome {
            agent: None,
            diagnostic: Some(diagnostic(
                "agent_missing_frontmatter",
                format!("Agent Markdown must include frontmatter: {path}"),
                path,
            )),
        };
    };
    let merged = parse_runtime_compatible_frontmatter(&frontmatter);
    let name = scalar_string(merged.get("name"));
    let description =
        scalar_string(merged.get("description")).map(|text| text.replace("\\n", "\n"));
    let Some(name) = name else {
        return missing_required("name", path);
    };
    let Some(description) = description else {
        return missing_required("description", path);
    };

    let source = if scope == SummaryScope::BuiltIn {
        "built-in"
    } else {
        "user"
    };
    let mut agent = Map::new();
    agent.insert(
        "id".to_owned(),
        Value::from(agent_state_id(&name, scope, source)),
    );
    agent.insert("name".to_owned(), Value::from(name));
    agent.insert("description".to_owned(), Value::from(description));
    agent.insert(
        "systemPrompt".to_owned(),
        Value::from(split.body.trim().to_owned()),
    );
    if let Some(color) = normalize_enum(merged.get("color"), &VALID_COLORS) {
        agent.insert("color".to_owned(), Value::from(color));
    }
    if let Some(selection) = parse_selection(&merged) {
        agent.insert("modelSelection".to_owned(), selection);
    }
    insert_list(&mut agent, "tools", tool_spec_list(merged.get("tools")));
    insert_list(
        &mut agent,
        "disallowedTools",
        tool_spec_list(merged.get("disallowedTools")),
    );
    insert_list(&mut agent, "skills", string_list(merged.get("skills")));
    if let Some(mode) = normalize_enum(merged.get("permissionMode"), &VALID_PERMISSION_MODES) {
        agent.insert("permissionMode".to_owned(), Value::from(mode));
    }
    if let Some(turns) = positive_integer(merged.get("maxTurns")) {
        agent.insert("maxTurns".to_owned(), Value::from(turns));
    }
    if let Some(background) = optional_boolean(merged.get("background")) {
        agent.insert("background".to_owned(), Value::Bool(background));
    }
    if let Some(inject) = optional_boolean(merged.get("injectAgentsMd")) {
        agent.insert("injectAgentsMd".to_owned(), Value::Bool(inject));
    }
    if let Some(Value::Array(servers)) = merged.get("mcpServers") {
        agent.insert("mcpServers".to_owned(), Value::Array(servers.clone()));
    }
    agent.insert("path".to_owned(), Value::from(path));
    agent.insert("scope".to_owned(), Value::from(scope.as_str()));
    agent.insert("source".to_owned(), Value::from(source));
    agent.insert("enabled".to_owned(), Value::Bool(true));
    agent.insert(
        "readOnly".to_owned(),
        Value::Bool(scope == SummaryScope::BuiltIn),
    );
    SummaryOutcome {
        agent: Some(Value::Object(agent)),
        diagnostic: None,
    }
}

fn missing_required(field: &str, path: &str) -> SummaryOutcome {
    SummaryOutcome {
        agent: None,
        diagnostic: Some(diagnostic(
            "agent_missing_required_frontmatter",
            format!("Agent frontmatter must include {field}: {path}"),
            path,
        )),
    }
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
