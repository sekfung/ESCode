//! ToolSearch 控制工具声明与描述文案。

use serde_json::{Value, json};
use std::collections::HashSet;

use super::tool_search_catalog::{
    DEFERRED_TOOL_CALL_NAME, ResolvedToolSearchMode, TOOL_SEARCH_NAME,
};

pub fn local_tool_search_description() -> &'static str {
    "Fetches full schema definitions for deferred tools so they can be called using DeferredToolCall.\n\n\
Deferred tools appear by name in system-reminder messages. Until fetched, only the name is known — there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete JSONSchema definitions inside a <functions> block. Call deferred tools through DeferredToolCall with {name, input}, using the schema from that result; call resident tools directly.\n\n\
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

/// Local 搜索结果：把命中的完整 `function` 定义渲染成上游的 `<functions>` 块。
/// 定义缺失的名字跳过；全部缺失时返回 None（不伪造 schema）。
pub fn schema_block(names: &[String], definitions: &[Value]) -> Option<String> {
    let functions = names
        .iter()
        .filter_map(|name| {
            let function = definitions
                .iter()
                .find(|definition| definition["function"]["name"].as_str() == Some(name))?
                .get("function")?;
            Some(format!(
                "<function>{}</function>",
                json!({
                    "description": function["description"],
                    "name": name,
                    "parameters": function["parameters"],
                })
            ))
        })
        .collect::<Vec<_>>();
    (!functions.is_empty()).then(|| format!("<functions>\n{}\n</functions>", functions.join("\n")))
}

/// 本次新加载工具的调用提醒（上游 `localToolsLoadedReminder`）。
pub fn local_loaded_reminder(names: &[String]) -> String {
    let action = if names.len() == 1 {
        "Call it now"
    } else {
        "Call them now"
    };
    reminder(format!(
        "Loaded: {}. {action} with {DEFERRED_TOOL_CALL_NAME} {{\"name\":\"<tool name>\",\"input\":{{...}}}}, using the exact tool name and filling input according to its schema above. No additional {TOOL_SEARCH_NAME} is needed before calling these tools.",
        names.join(", ")
    ))
}

/// 会话内已加载工具的提醒（上游 `localToolsAlreadyLoadedReminder`）。
pub fn local_already_loaded_reminder(names: &[String]) -> String {
    reminder(format!(
        "Tool definitions already available in this conversation: {}. Use {DEFERRED_TOOL_CALL_NAME} with {{name, input}} to execute these tools. Searching again does not execute them.",
        names.join(", ")
    ))
}

fn reminder(body: String) -> String {
    format!("<system-reminder>\n{body}\n</system-reminder>")
}

/// local ToolSearch 结果的模型可见 part：定义块 + 新加载提醒 + 会话内已加载提醒。
/// `visible` 是本批调用开始时的已发现快照（上游 `visible`）。
pub fn local_media(
    matches: &[String],
    definitions: &[Value],
    deferred: &HashSet<String>,
    visible: &HashSet<String>,
) -> Vec<Value> {
    // 上游 projectToolReferences：目录里不存在的引用直接丢弃；已可见的延迟工具只给
    // “already available”提醒、不再展开 schema；常驻工具始终展开且不参与两种提醒。
    let mut schemas = Vec::new();
    let mut fresh = Vec::new();
    let mut already = Vec::new();
    for name in matches {
        if !has_definition(definitions, name) {
            continue;
        }
        let is_deferred = deferred.contains(name);
        if is_deferred && visible.contains(name) {
            already.push(name.clone());
            continue;
        }
        schemas.push(name.clone());
        if is_deferred {
            fresh.push(name.clone());
        }
    }
    let mut parts = Vec::new();
    if let Some(block) = schema_block(&schemas, definitions) {
        parts.push(json!({"type": "text", "text": block}));
    }
    if !fresh.is_empty() {
        parts.push(json!({"type": "text", "text": local_loaded_reminder(&fresh)}));
    }
    if !already.is_empty() {
        parts.push(json!({"type": "text", "text": local_already_loaded_reminder(&already)}));
    }
    parts
}

fn has_definition(definitions: &[Value], name: &str) -> bool {
    definitions
        .iter()
        .any(|definition| definition["function"]["name"].as_str() == Some(name))
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
    use std::collections::HashSet;

    #[test]
    fn schema_block_renders_matched_definitions_only() {
        let definitions = vec![
            json!({"type":"function","function":{"name":"mcp__a__echo","description":"Echo","parameters":{"type":"object"}}}),
            json!({"type":"function","function":{"name":"Read","description":"Read","parameters":{"type":"object"}}}),
        ];
        let block = schema_block(&["mcp__a__echo".to_owned()], &definitions).unwrap();
        assert!(block.starts_with("<functions>\n<function>"));
        assert!(block.contains("\"name\":\"mcp__a__echo\""));
        assert!(block.ends_with("\n</functions>"));
        assert!(!block.contains("Read"));
    }

    #[test]
    fn schema_block_skips_unknown_names() {
        assert!(schema_block(&["missing".to_owned()], &[]).is_none());
    }

    #[test]
    fn local_loaded_reminder_matches_upstream_copy() {
        assert_eq!(
            local_loaded_reminder(&["mcp__a__echo".to_owned()]),
            "<system-reminder>\nLoaded: mcp__a__echo. Call it now with DeferredToolCall \
{\"name\":\"<tool name>\",\"input\":{...}}, using the exact tool name and filling input according \
to its schema above. No additional ToolSearch is needed before calling these tools.\n</system-reminder>"
        );
        assert!(
            local_loaded_reminder(&["a".to_owned(), "b".to_owned()])
                .contains("Loaded: a, b. Call them now")
        );
    }

    #[test]
    fn local_already_loaded_reminder_matches_upstream_copy() {
        assert_eq!(
            local_already_loaded_reminder(&["mcp__a__echo".to_owned()]),
            "<system-reminder>\nTool definitions already available in this conversation: \
mcp__a__echo. Use DeferredToolCall with {name, input} to execute these tools. Searching again \
does not execute them.\n</system-reminder>"
        );
    }

    #[test]
    fn local_media_projects_fresh_already_resident_and_missing_tools() {
        let definitions = vec![
            json!({"type":"function","function":{"name":"A","description":"a","parameters":{}}}),
            json!({"type":"function","function":{"name":"B","description":"b","parameters":{}}}),
            json!({"type":"function","function":{"name":"Resident","description":"r","parameters":{}}}),
        ];
        let deferred: HashSet<String> = ["A", "B"].iter().map(|name| (*name).to_owned()).collect();
        let names = |values: &[&str]| values.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        let text = |part: &Value| part["text"].as_str().unwrap_or_default().to_owned();

        let first = local_media(&names(&["A", "B"]), &definitions, &deferred, &HashSet::new());
        assert!(text(&first[0]).contains("\"name\":\"A\""));
        assert!(text(&first[1]).starts_with("<system-reminder>\nLoaded: A, B."));

        let visible: HashSet<String> = ["A"].iter().map(|name| (*name).to_owned()).collect();
        let mixed = local_media(&names(&["A", "B"]), &definitions, &deferred, &visible);
        assert!(!text(&mixed[0]).contains("\"name\":\"A\""));
        assert!(text(&mixed[0]).contains("\"name\":\"B\""));
        assert!(text(&mixed[1]).contains("Loaded: B."));
        assert!(text(&mixed[2]).contains("already available in this conversation: A."));

        let repeat = local_media(&names(&["A", "B"]), &definitions, &deferred, &deferred);
        assert_eq!(repeat.len(), 1);
        assert!(text(&repeat[0]).contains("already available in this conversation: A, B."));

        let resident = local_media(
            &names(&["Resident", "missing"]),
            &definitions,
            &deferred,
            &deferred,
        );
        assert_eq!(resident.len(), 1);
        assert!(text(&resident[0]).contains("\"name\":\"Resident\""));
    }
}
