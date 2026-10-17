//! agent markdown → `AgentProfile`（TS `subagent-profile-parser.ts` + `subagent-markdown-selection.ts`
//! 的**解析侧**；序列化与 legacy provider 迁移不在本片）。
//!
//! 诊断口径与 TS 逐字：缺 frontmatter / 缺 name / 缺 description 直接失败；`memory` 非法是
//! **带 profile 的警告**；`mcpServers` 非法整条失败。项目级来源**不允许** `permissionMode`
//! （仓库输入不能把 child runtime 提升到 bypass）。

use serde_json::{Map, Value, json};

use super::frontmatter::{parse_agent_frontmatter, split_markdown_frontmatter};
use super::model_value::{decode_custom_model_value, parse_model_picker_value};

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
const VALID_MEMORY_SCOPES: [&str; 3] = ["user", "project", "local"];
const INHERIT_NAMES: [&str; 5] = ["inherit", "main", "sonnet", "opus", "haiku"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    BuiltIn,
    Project,
    User,
}

impl ProfileSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

/// TS `AgentProfileParseDiagnostic`。
#[derive(Debug)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub path: Option<String>,
}

impl Diagnostic {
    pub fn to_json(&self) -> Value {
        let mut value = json!({"code": self.code, "message": self.message});
        if let Some(path) = &self.path {
            value["path"] = Value::from(path.clone());
        }
        value
    }
}

/// 解析成功的结果：profile 一定在，诊断可能是警告（`agent_invalid_memory_scope`）。
#[derive(Debug)]
pub struct ParsedProfile {
    pub profile: Value,
    pub diagnostic: Option<Diagnostic>,
}

fn inline_path(path: Option<&str>) -> &str {
    path.unwrap_or("<inline>")
}

/// TS `parseAgentProfileFromMarkdown`。
pub fn parse_agent_profile_from_markdown(
    content: &str,
    path: Option<&str>,
    source: ProfileSource,
) -> Result<ParsedProfile, Diagnostic> {
    let split = split_markdown_frontmatter(content);
    let Some(frontmatter) = split.frontmatter else {
        return Err(Diagnostic {
            code: "agent_missing_frontmatter",
            message: format!(
                "Agent Markdown must include frontmatter: {}",
                inline_path(path)
            ),
            path: path.map(str::to_owned),
        });
    };
    let parsed = parse_agent_frontmatter(&frontmatter);
    let values = parsed.values;
    let name = scalar_string(values.get("name"));
    let description =
        scalar_string(values.get("description")).map(|text| text.replace("\\n", "\n"));
    let Some(name) = name else {
        return Err(missing_required("name", path));
    };
    let Some(description) = description else {
        return Err(missing_required("description", path));
    };
    if matches!(parsed.mcp_servers, Some(None)) {
        return Err(Diagnostic {
            code: "agent_invalid_mcp_servers",
            message: format!(
                "Agent frontmatter mcpServers must be a list of parent server names: {}",
                inline_path(path)
            ),
            path: path.map(str::to_owned),
        });
    }

    let model_selection = parse_selection(&values);
    let color =
        scalar_string(values.get("color")).filter(|value| VALID_COLORS.contains(&value.as_str()));
    let permission_mode = scalar_string(values.get("permissionMode")).filter(|value| {
        source != ProfileSource::Project && VALID_PERMISSION_MODES.contains(&value.as_str())
    });
    let max_turns = normalize_positive_integer(values.get("maxTurns"));
    let memory = scalar_string(values.get("memory"))
        .filter(|value| VALID_MEMORY_SCOPES.contains(&value.as_str()));
    let memory_diagnostic =
        (values.contains_key("memory") && memory.is_none()).then(|| Diagnostic {
            code: "agent_invalid_memory_scope",
            message: format!(
                "Agent frontmatter memory must be user, project, or local: {}",
                inline_path(path)
            ),
            path: path.map(str::to_owned),
        });

    let mut profile = Map::new();
    profile.insert("name".to_owned(), Value::from(name));
    profile.insert("description".to_owned(), Value::from(description));
    profile.insert("source".to_owned(), Value::from(source.as_str()));
    profile.insert(
        "systemPrompt".to_owned(),
        Value::from(split.body.trim().to_owned()),
    );
    if let Some(path) = path {
        profile.insert("path".to_owned(), Value::from(path.to_owned()));
    }
    if let Some(selection) = model_selection {
        profile.insert("modelSelection".to_owned(), selection);
    }
    if let Some(color) = color {
        profile.insert("color".to_owned(), Value::from(color));
    }
    if let Some(mode) = permission_mode {
        profile.insert("permissionMode".to_owned(), Value::from(mode));
    }
    if let Some(turns) = max_turns {
        profile.insert("maxTurns".to_owned(), Value::from(turns));
    }
    if let Some(memory) = memory {
        profile.insert("memory".to_owned(), Value::from(memory));
    }
    if let Some(tools) = optional_list("tools", values.get("tools")) {
        profile.insert("tools".to_owned(), tools);
    }
    if let Some(tools) = optional_list("disallowedTools", values.get("disallowedTools")) {
        profile.insert("disallowedTools".to_owned(), tools);
    }
    if let Some(skills) = optional_list("skills", values.get("skills")) {
        profile.insert("skills".to_owned(), skills);
    }
    if let Some(background) = optional_boolean(values.get("background")) {
        profile.insert("background".to_owned(), Value::Bool(background));
    }
    if let Some(inject) = optional_boolean(values.get("injectAgentsMd")) {
        profile.insert("injectAgentsMd".to_owned(), Value::Bool(inject));
    }
    if let Some(Some(servers)) = parsed.mcp_servers {
        profile.insert(
            "mcpServers".to_owned(),
            Value::Array(servers.into_iter().map(Value::from).collect()),
        );
    }
    Ok(ParsedProfile {
        profile: Value::Object(profile),
        diagnostic: memory_diagnostic,
    })
}

fn missing_required(field: &str, path: Option<&str>) -> Diagnostic {
    Diagnostic {
        code: "agent_missing_required_frontmatter",
        message: format!(
            "Agent frontmatter must include {field}: {}",
            inline_path(path)
        ),
        path: path.map(str::to_owned),
    }
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn normalize_positive_integer(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64().filter(|turns| *turns > 0),
        Some(Value::String(text)) => text.trim().parse::<u64>().ok().filter(|turns| *turns > 0),
        _ => None,
    }
}

fn optional_boolean(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::String(text)) => {
            let lower = text.to_ascii_lowercase();
            if lower == "true" {
                Some(true)
            } else if lower == "false" {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// TS `optionalList`：列表非空才带上；`tools` 写成数组但解析为空时显式回 `[]`。
fn optional_list(key: &str, value: Option<&Value>) -> Option<Value> {
    let list = if key == "skills" {
        normalize_string_list(value)
    } else {
        parse_tool_list(value)
    };
    if let Some(list) = list.filter(|list| !list.is_empty()) {
        return Some(Value::Array(list.into_iter().map(Value::from).collect()));
    }
    if key == "tools" && value.is_some_and(Value::is_array) {
        return Some(Value::Array(Vec::new()));
    }
    None
}

fn normalize_string_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(items)) => {
            let list: Vec<String> = items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect();
            (!list.is_empty()).then_some(list)
        }
        Some(Value::String(text)) => {
            let list: Vec<String> = text
                .split([',', ' ', '\t', '\n', '\r'])
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect();
            (!list.is_empty()).then_some(list)
        }
        _ => None,
    }
}

/// 字符串形态按「括号深度 0 的逗号或空白」切分，再逐个去 `(...)` 后缀。
fn parse_tool_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(_)) => normalize_tool_names(normalize_string_list(value)),
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
            normalize_tool_names(Some(result))
        }
        _ => None,
    }
}

fn normalize_tool_names(values: Option<Vec<String>>) -> Option<Vec<String>> {
    let values = values?;
    let names: Vec<String> = values
        .iter()
        .map(|value| match value.find('(') {
            Some(index) => value[..index].trim().to_owned(),
            None => value.trim().to_owned(),
        })
        .filter(|name| !name.is_empty())
        .collect();
    (!names.is_empty()).then_some(names)
}

/// TS `parseSubagentMarkdownSelection`：`model` 是 `provider/model[$reasoning]` 或 `custom:...`；
/// `inherit`/`main`/`sonnet`/`opus`/`haiku` 视为继承（不产生覆盖）。
pub(super) fn parse_selection(frontmatter: &Map<String, Value>) -> Option<Value> {
    let model = frontmatter.get("model")?.as_str()?;
    let value = model.trim();
    if value.is_empty() || INHERIT_NAMES.contains(&value) {
        return None;
    }
    let mut selection = match decode_custom_model_value(value) {
        Some(custom) => {
            let provider = custom.0.trim();
            let name = custom.1.as_deref().map(str::trim).unwrap_or_default();
            if provider.is_empty() || name.is_empty() {
                return None;
            }
            json!({"providerId": provider, "modelId": name})
        }
        None => parse_model_picker_value(value)?,
    };
    let reasoning = frontmatter
        .get("thoughtLevel")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !reasoning.is_empty() {
        selection["options"] = json!({"reasoningLevel": reasoning});
    }
    Some(selection)
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
