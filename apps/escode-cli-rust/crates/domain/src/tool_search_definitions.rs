//! ToolSearch 控制工具声明与描述文案。

use serde_json::{Value, json};

use super::tool_search_catalog::{
    DEFERRED_TOOL_CALL_NAME, ResolvedToolSearchMode, TOOL_SEARCH_NAME,
};

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
