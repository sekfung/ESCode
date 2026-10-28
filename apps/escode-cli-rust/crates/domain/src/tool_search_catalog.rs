//! ToolSearch / DeferredToolCall 的目录、搜索与调用解析
//! （docs/specs/rust-tool-search.md，对齐上游 v3.16.2 `tool/search/*` 与 `tool/deferred-call.ts`）。
//!
//! 这里只做纯函数：谁被延迟、搜索怎么排序、DeferredToolCall 是否允许解包，
//! 不读文件、不发网络请求，也不持有会话状态。

use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashSet;

pub const TOOL_SEARCH_NAME: &str = "ToolSearch";
pub const DEFERRED_TOOL_CALL_NAME: &str = "DeferredToolCall";
pub const TOOL_SEARCH_DEFAULT_RESULTS: usize = 5;
pub const TOOL_SEARCH_BETA: &str = "advanced-tool-use-2025-11-20";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSearchMode {
    Off,
    Local,
    Native,
    Auto,
}

impl ToolSearchMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "local" => Some(Self::Local),
            "native" => Some(Self::Native),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ToolSearchCapability {
    pub protocol_supported: bool,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedToolSearchMode {
    Off,
    Local,
    Native,
}

impl ResolvedToolSearchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Local => "local",
            Self::Native => "native",
        }
    }
}

/// 与上游 `resolveToolSearchMode` 一致：native 只允许声明协议支持的 provider；
/// auto 在模型默认启用时走 native，否则有工具调用能力就走 local。
pub fn resolve_tool_search_mode(
    mode: ToolSearchMode,
    capability: ToolSearchCapability,
    supports_tool_call: bool,
) -> Result<ResolvedToolSearchMode, String> {
    match mode {
        ToolSearchMode::Off => Ok(ResolvedToolSearchMode::Off),
        ToolSearchMode::Native => {
            if !capability.protocol_supported {
                return Err(
                    "tool search mode native requires an Anthropic Messages provider".into(),
                );
            }
            Ok(ResolvedToolSearchMode::Native)
        }
        ToolSearchMode::Auto => {
            if capability.protocol_supported && capability.default_enabled {
                Ok(ResolvedToolSearchMode::Native)
            } else if supports_tool_call {
                Ok(ResolvedToolSearchMode::Local)
            } else {
                Ok(ResolvedToolSearchMode::Off)
            }
        }
        ToolSearchMode::Local => {
            if supports_tool_call {
                Ok(ResolvedToolSearchMode::Local)
            } else {
                Err("tool search mode local requires a provider that supports tool calls".into())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPresentation {
    pub server_name: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchCandidate {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: Option<String>,
    pub search_hint: Option<String>,
    pub mcp: Option<McpPresentation>,
    pub deferred: bool,
}

impl ToolSearchCandidate {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            aliases: Vec::new(),
            description: None,
            search_hint: None,
            mcp: None,
            deferred: false,
        }
    }
}

/// 模型可见的 MCP 名称片段（上游 `toModelVisibleMcpNamePart`）。
pub fn to_model_visible_mcp_name_part(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let collapsed = sanitized
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if collapsed.is_empty() {
        "unknown".into()
    } else {
        collapsed
    }
}

fn parse_mcp_name(name: &str) -> Option<McpPresentation> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(McpPresentation {
        server_name: server.into(),
        tool_name: tool.into(),
    })
}

/// 从发给模型的 `{"type":"function","function":{...}}` 定义构造目录项。
/// 延迟策略与上游 `isDeferredTool` 的可见部分一致：MCP 工具与 Task 别名延迟，
/// 控制工具本身永不延迟。
pub fn candidate_from_definition(definition: &Value) -> Option<ToolSearchCandidate> {
    let function = &definition["function"];
    let name = function["name"].as_str()?.to_string();
    if name == TOOL_SEARCH_NAME || name == DEFERRED_TOOL_CALL_NAME {
        return None;
    }
    let mcp = parse_mcp_name(&name);
    let deferred = mcp.is_some() || name == "Task";
    Some(ToolSearchCandidate {
        name,
        aliases: Vec::new(),
        description: function["description"].as_str().map(str::to_owned),
        search_hint: function["searchHint"].as_str().map(str::to_owned),
        mcp,
        deferred,
    })
}

struct Tokenized {
    coarse: Vec<String>,
    parts: Vec<String>,
    full: String,
    mcp: bool,
}

fn tokenize(tool: &ToolSearchCandidate) -> Tokenized {
    let name = tool.name.as_str();
    let mcp_parts = tool
        .mcp
        .as_ref()
        .map(|m| (m.server_name.clone(), m.tool_name.clone()))
        .or_else(|| parse_mcp_name(name).map(|m| (m.server_name, m.tool_name)));
    if let Some((server, tool_name)) = mcp_parts {
        let coarse = vec![server.to_ascii_lowercase(), tool_name.to_ascii_lowercase()];
        let parts: Vec<String> = coarse
            .iter()
            .flat_map(|s| {
                s.split([' ', '_', '.'])
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        let full = parts.join(" ");
        return Tokenized {
            coarse,
            parts,
            full,
            mcp: true,
        };
    }
    let coarse = vec![name.to_ascii_lowercase()];
    let mut spaced = String::with_capacity(name.len() + 8);
    let mut previous_lower = false;
    for c in name.chars() {
        if previous_lower && c.is_ascii_uppercase() {
            spaced.push(' ');
        }
        previous_lower = c.is_ascii_lowercase();
        spaced.push(c);
    }
    let parts: Vec<String> = spaced
        .replace('_', " ")
        .to_ascii_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let full = parts.join(" ");
    Tokenized {
        coarse,
        parts,
        full,
        mcp: false,
    }
}

fn resolve_name<'a>(
    name: &str,
    catalog: &'a [ToolSearchCandidate],
) -> Option<&'a ToolSearchCandidate> {
    catalog
        .iter()
        .find(|tool| tool.name == name || tool.aliases.iter().any(|alias| alias == name))
}

fn escape_regex(term: &str) -> String {
    term.chars()
        .flat_map(|c| {
            if "\\.+*?()|[]{}^$".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

/// 上游 `searchTools` 的等价实现。返回按相关度降序的工具名。
pub fn search_tools(
    query: &str,
    catalog: &[ToolSearchCandidate],
    max_results: usize,
) -> Vec<String> {
    let max_results = max_results.max(1);
    let deferred: Vec<&ToolSearchCandidate> = catalog.iter().filter(|tool| tool.deferred).collect();

    if let Some(select) = query
        .strip_prefix("select:")
        .or_else(|| query.strip_prefix("SELECT:"))
    {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for value in select.split(',') {
            let name = value.trim();
            if name.is_empty() {
                continue;
            }
            if let Some(exact) = deferred
                .iter()
                .copied()
                .find(|tool| tool.name == name || tool.aliases.iter().any(|alias| alias == name))
                .or_else(|| resolve_name(name, catalog))
            {
                if seen.insert(exact.name.clone()) {
                    names.push(exact.name.clone());
                }
                continue;
            }
            for tool in catalog {
                let Some(mcp) = tool.mcp.as_ref() else {
                    continue;
                };
                let visible = to_model_visible_mcp_name_part(&mcp.tool_name);
                if (mcp.tool_name == name || visible == name) && seen.insert(tool.name.clone()) {
                    names.push(tool.name.clone());
                }
            }
        }
        return names;
    }

    let text = query.to_ascii_lowercase();
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if let Some(exact) = deferred
        .iter()
        .copied()
        .find(|tool| tool.name.to_ascii_lowercase() == text)
        .or_else(|| {
            catalog
                .iter()
                .find(|tool| tool.name.to_ascii_lowercase() == text)
        })
    {
        return vec![exact.name.clone()];
    }
    if text.starts_with("mcp__") && text.len() > 5 {
        let names: Vec<String> = deferred
            .iter()
            .filter(|tool| tool.name.to_ascii_lowercase().starts_with(text))
            .take(max_results)
            .map(|tool| tool.name.clone())
            .collect();
        if !names.is_empty() {
            return names;
        }
    }

    let terms: Vec<&str> = text
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect();
    let required: Vec<&str> = terms
        .iter()
        .filter(|term| term.starts_with('+') && term.len() > 1)
        .map(|term| &term[1..])
        .collect();
    let keywords: Vec<&str> = if required.is_empty() {
        terms.clone()
    } else {
        required
            .iter()
            .copied()
            .chain(
                terms
                    .iter()
                    .copied()
                    .filter(|term| !(term.starts_with('+') && term.len() > 1)),
            )
            .collect()
    };
    if keywords.is_empty() {
        return Vec::new();
    }
    let patterns: std::collections::HashMap<&str, Regex> = keywords
        .iter()
        .map(|term| {
            let pattern = Regex::new(&format!(r"\b{}\b", escape_regex(term)))
                .expect("escaped keyword compiles");
            (*term, pattern)
        })
        .collect();

    let mut scored: Vec<(String, i64)> = deferred
        .iter()
        .map(|tool| {
            let name = tokenize(tool);
            let description = tool
                .description
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let hint = tool
                .search_hint
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if !required.iter().all(|term| {
                name.parts.iter().any(|part| part.contains(term))
                    || name.coarse.iter().any(|part| part.contains(term))
                    || patterns
                        .get(term)
                        .is_some_and(|pattern| pattern.is_match(&description))
                    || patterns
                        .get(term)
                        .is_some_and(|pattern| pattern.is_match(&hint))
            }) {
                return (tool.name.clone(), 0);
            }
            let mut score = 0i64;
            for term in &keywords {
                if name.parts.iter().any(|part| part == term) {
                    score += if name.mcp { 12 } else { 10 };
                } else if name.parts.iter().any(|part| part.contains(term)) {
                    score += if name.mcp { 6 } else { 5 };
                }
                if name.coarse.iter().any(|part| part == term) {
                    score += if name.mcp { 12 } else { 10 };
                } else if name.coarse.iter().any(|part| part.contains(term)) {
                    score += if name.mcp { 4 } else { 3 };
                }
                if name.full.contains(term) && score == 0 {
                    score += 3;
                }
                if !hint.is_empty()
                    && patterns
                        .get(term)
                        .is_some_and(|pattern| pattern.is_match(&hint))
                {
                    score += 4;
                }
                if patterns
                    .get(term)
                    .is_some_and(|pattern| pattern.is_match(&description))
                {
                    score += 2;
                }
            }
            (tool.name.clone(), score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    scored
        .into_iter()
        .take(max_results)
        .map(|(name, _)| name)
        .collect()
}

/// DeferredToolCall 解包：只接受已发现且确实延迟的目标工具。
/// 返回 `(target_name, target_input)`；错误文案直接交回模型。
pub fn resolve_deferred_call(
    name: &str,
    input: &Value,
    catalog: &[ToolSearchCandidate],
    discovered: &HashSet<String>,
    mode: ResolvedToolSearchMode,
) -> Result<(String, Value), String> {
    if mode != ResolvedToolSearchMode::Local {
        return Err("DeferredToolCall is not available for this request.".into());
    }
    if name.is_empty() || !input.is_object() {
        return Err("Invalid DeferredToolCall input.".into());
    }
    let candidate = catalog
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| format!("Tool not available in this request: {name}"))?;
    if name == TOOL_SEARCH_NAME || name == DEFERRED_TOOL_CALL_NAME || !candidate.deferred {
        return Err(format!(
            "Call {name} directly using its normal tool call; DeferredToolCall only accepts deferred tools."
        ));
    }
    if !discovered.contains(name) {
        return Err(format!(
            "Tool schema for {name} is not loaded. Call ToolSearch with query \"select:{name}\" first."
        ));
    }
    Ok((candidate.name.clone(), input.clone()))
}

/// 包装边界：DeferredToolCall 的目标结果在模型历史/持久化里保留包装名，
/// 供 UI 与冷恢复区分「直接调用」与「经包装入口调用」。
pub fn model_call_wrapper(name: &str) -> Option<&'static str> {
    (name == DEFERRED_TOOL_CALL_NAME).then_some(DEFERRED_TOOL_CALL_NAME)
}

/// Native 模式的模型可见结果：每个命中一个 `tool_reference` part（camelCase 内部形状，
/// 由 model 协议层投影成 provider 的 `tool_reference`/`tool_name`）。
pub fn tool_reference_blocks(matches: &[String]) -> Vec<Value> {
    matches
        .iter()
        .map(|name| json!({"type": "tool_reference", "toolName": name}))
        .collect()
}

pub fn local_tool_search_description() -> &'static str {
    "Fetches full schema definitions for deferred tools so they can be called using DeferredToolCall.\n\n\
Deferred tools appear by name in system-reminder messages. Until fetched, only the name is known — there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' names. Call deferred tools through DeferredToolCall with {name, input}; call resident tools directly.\n\n\
Query forms:\n\
- \"select:Read,Edit,Grep\" — fetch these exact tools by name\n\
- \"notebook jupyter\" — keyword search, up to max_results best matches\n\
- \"+slack send\" — require \"slack\" in the name, rank by remaining terms"
}

/// Native 模式的描述：schema 由 provider 展开，客户端不提供 DeferredToolCall。
pub fn native_tool_search_description() -> &'static str {
    "Fetches full schema definitions for deferred tools so they can be called.\n\n\
Deferred tools appear by name in system-reminder messages. Until fetched, only the name is known — there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete JSONSchema definitions inside a <functions> block. Once a tool's schema appears in that result, it is callable exactly like any tool defined at the top of the prompt.\n\n\
Query forms:\n\
- \"select:Read,Edit,Grep\" — fetch these exact tools by name\n\
- \"notebook jupyter\" — keyword search, up to max_results best matches\n\
- \"+slack send\" — require \"slack\" in the name, rank by remaining terms"
}

pub fn deferred_tool_call_description() -> &'static str {
    "Immediately call one deferred tool and return its result. Use ToolSearch to load its complete definition, then pass its exact name and input object. Only deferred tools can be called here; call resident tools directly using their normal tool calls."
}

/// 控制工具的模型可见定义；只在模式启用时注入。
pub fn control_definitions(mode: ResolvedToolSearchMode) -> Vec<Value> {
    if mode == ResolvedToolSearchMode::Off {
        return Vec::new();
    }
    let mut definitions = vec![json!({
        "type": "function",
        "function": {
            "name": TOOL_SEARCH_NAME,
            "description": match mode {
                ResolvedToolSearchMode::Native => native_tool_search_description(),
                _ => local_tool_search_description(),
            },
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Query to find deferred tools. Use \"select:<tool_name>\" for direct selection, or keywords to search."
                    },
                    "max_results": {
                        "type": "number",
                        "description": "Maximum number of results to return (default: 5)"
                    }
                },
                "required": ["query", "max_results"],
                "additionalProperties": false
            }
        }
    })];
    if mode == ResolvedToolSearchMode::Local {
        definitions.push(json!({
            "type": "function",
            "function": {
                "name": DEFERRED_TOOL_CALL_NAME,
                "description": deferred_tool_call_description(),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "minLength": 1},
                        "input": {"type": "object"}
                    },
                    "required": ["name", "input"],
                    "additionalProperties": false
                }
            }
        }));
    }
    definitions
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, description: &str, deferred: bool) -> ToolSearchCandidate {
        let mut candidate = ToolSearchCandidate::new(name);
        candidate.description = Some(description.into());
        candidate.mcp = parse_mcp_name(name);
        candidate.deferred = deferred;
        candidate
    }

    #[test]
    fn resolves_modes_like_upstream() {
        let capability = ToolSearchCapability {
            protocol_supported: true,
            default_enabled: true,
        };
        assert_eq!(
            resolve_tool_search_mode(ToolSearchMode::Auto, capability, true).unwrap(),
            ResolvedToolSearchMode::Native
        );
        assert_eq!(
            resolve_tool_search_mode(ToolSearchMode::Auto, ToolSearchCapability::default(), true)
                .unwrap(),
            ResolvedToolSearchMode::Local
        );
        assert!(
            resolve_tool_search_mode(
                ToolSearchMode::Native,
                ToolSearchCapability::default(),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn select_accepts_exact_alias_and_mcp_tool_name() {
        let mut alias = tool("Task", "alias", true);
        alias.aliases.push("AgentAlias".into());
        let catalog = vec![
            alias,
            tool("mcp__github__create_issue", "issues", true),
            tool("Read", "read files", false),
        ];
        assert_eq!(
            search_tools("select:AgentAlias", &catalog, 5),
            vec!["Task".to_string()]
        );
        assert_eq!(
            search_tools("select:create_issue", &catalog, 5),
            vec!["mcp__github__create_issue".to_string()]
        );
        assert!(!search_tools("select:Read", &catalog, 5).is_empty());
    }

    #[test]
    fn keyword_search_ranks_name_matches_and_honours_required_terms() {
        let catalog = vec![
            tool("mcp__slack__send_message", "send a message", true),
            tool("mcp__github__create_issue", "create an issue", true),
            tool("mcp__github__list_issues", "list issues", true),
        ];
        let names = search_tools("+slack send", &catalog, 5);
        assert_eq!(names, vec!["mcp__slack__send_message".to_string()]);
        let names = search_tools("issue", &catalog, 1);
        assert_eq!(names.len(), 1);
        assert!(names[0].contains("issue"));
    }

    #[test]
    fn keyword_search_only_returns_deferred_tools_but_exact_selection_sees_resident() {
        let catalog = vec![
            tool("Read", "read files", false),
            tool("mcp__github__create_issue", "create an issue", true),
        ];
        // 上游：select/精确名可以命中 resident（用于直接选择），关键词搜索只看 deferred。
        assert_eq!(search_tools("read", &catalog, 5), vec!["Read".to_string()]);
        assert!(
            search_tools("issue", &catalog, 5)
                .iter()
                .all(|name| name.starts_with("mcp__"))
        );
    }

    #[test]
    fn deferred_call_requires_discovery_and_deferred_target() {
        let catalog = vec![
            tool("mcp__github__create_issue", "create an issue", true),
            tool("Read", "read files", false),
        ];
        let mut discovered = HashSet::new();
        assert!(
            resolve_deferred_call(
                "mcp__github__create_issue",
                &json!({}),
                &catalog,
                &discovered,
                ResolvedToolSearchMode::Local
            )
            .is_err()
        );
        discovered.insert("mcp__github__create_issue".into());
        assert_eq!(
            resolve_deferred_call(
                "mcp__github__create_issue",
                &json!({"title":"x"}),
                &catalog,
                &discovered,
                ResolvedToolSearchMode::Local
            )
            .unwrap(),
            (
                "mcp__github__create_issue".to_string(),
                json!({"title":"x"})
            )
        );
        assert!(
            resolve_deferred_call(
                "mcp__github__create_issue",
                &json!({}),
                &catalog,
                &discovered,
                ResolvedToolSearchMode::Off
            )
            .is_err()
        );
    }

    #[test]
    fn control_tools_are_only_present_when_enabled() {
        assert!(control_definitions(ResolvedToolSearchMode::Off).is_empty());
        assert_eq!(control_definitions(ResolvedToolSearchMode::Local).len(), 2);
        assert_eq!(control_definitions(ResolvedToolSearchMode::Native).len(), 1);
    }

    #[test]
    fn model_call_wrapper_marks_only_deferred_wrapper() {
        assert_eq!(
            model_call_wrapper(DEFERRED_TOOL_CALL_NAME),
            Some(DEFERRED_TOOL_CALL_NAME)
        );
        assert_eq!(model_call_wrapper("mcp__a__b"), None);
    }

    #[test]
    fn native_tool_reference_blocks_use_internal_camel_case() {
        let blocks = tool_reference_blocks(&["mcp__a__b".to_string()]);
        assert_eq!(
            blocks,
            vec![json!({"type":"tool_reference","toolName":"mcp__a__b"})]
        );
    }

    #[test]
    fn mcp_name_sanitizes_like_upstream() {
        assert_eq!(to_model_visible_mcp_name_part("a.b c"), "a_b_c");
        assert_eq!(to_model_visible_mcp_name_part(""), "unknown");
    }
}
