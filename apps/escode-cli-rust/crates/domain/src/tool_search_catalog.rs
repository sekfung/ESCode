//! ToolSearch / DeferredToolCall 的目录、搜索与调用解析
//! （docs/specs/rust-tool-search.md，对齐上游 v3.16.2 `tool/search/*` 与 `tool/deferred-call.ts`）。
//!
//! 这里只做纯函数：谁被延迟、搜索怎么排序、DeferredToolCall 是否允许解包，
//! 不读文件、不发网络请求，也不持有会话状态。

use serde_json::{Value, json};
use std::collections::HashSet;

pub use super::tool_search_definitions::{
    control_definitions, deferred_tool_call_description, local_tool_search_description,
    native_tool_search_description,
};
pub use super::tool_search_query::search_tools;

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

pub(crate) fn parse_mcp_name(name: &str) -> Option<McpPresentation> {
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
