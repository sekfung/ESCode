//! ToolSearch 引用的跨模式投影（docs/specs/rust-tool-search.md）。
//!
//! `tool_reference` 只在 Anthropic native 模式的工具结果里有线上意义。会话从 native 切到
//! local/off 或另一家 provider 后，历史里仍可能保留引用块；发送前必须降级为文本，
//! 否则 provider 会因未知 part 拒绝整条请求（上游 `projectToolReferences` 的同一职责）。

use serde_json::{Value, json};
use std::collections::HashSet;

/// 非 native：把 `tool_reference` 降级为文本；native：保留仍然可用的引用，
/// 删除本轮目录里已不存在的工具引用（上游 `projectToolReferences`）。
pub(super) fn project(
    messages: &mut [Value],
    native_tool_search: bool,
    available: &HashSet<String>,
) {
    if native_tool_search {
        prune_unavailable(messages, available);
        return;
    }
    for message in messages {
        if message["role"] != "tool" {
            continue;
        }
        let Some(parts) = message["content"].as_array_mut() else {
            continue;
        };
        for part in parts.iter_mut() {
            if part["type"] == "tool_reference" {
                let name = part["toolName"].as_str().unwrap_or_default();
                *part = json!({
                    "type": "text",
                    "text": format!("Loaded tool schema: {name}"),
                });
            }
        }
    }
}

fn prune_unavailable(messages: &mut [Value], available: &HashSet<String>) {
    for message in messages {
        if message["role"] != "tool" {
            continue;
        }
        let Some(parts) = message["content"].as_array_mut() else {
            continue;
        };
        let before = parts.len();
        parts.retain(|part| {
            part["type"] != "tool_reference"
                || part["toolName"]
                    .as_str()
                    .is_some_and(|name| available.contains(name))
        });
        if before > 0 && parts.is_empty() {
            // 上游在引用全部被移除时写入占位文本，保持 tool_result 非空。
            message["content"] = json!([{
                "type": "text",
                "text": "[Tool references removed - tools no longer available]",
            }]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn downgrades_references_when_native_is_off() {
        let mut messages = vec![json!({
            "role": "tool",
            "content": [{"type":"tool_reference","toolName":"mcp__a__b"}]
        })];
        project(&mut messages, false, &available(&[]));
        assert_eq!(
            messages[0]["content"],
            json!([{"type":"text","text":"Loaded tool schema: mcp__a__b"}])
        );
    }

    #[test]
    fn keeps_references_when_native_is_on() {
        let mut messages = vec![
            json!({"role":"tool","content":[{"type":"tool_reference","toolName":"mcp__a__b"}]}),
        ];
        project(&mut messages, true, &available(&["mcp__a__b"]));
        assert_eq!(
            messages[0]["content"],
            json!([{"type":"tool_reference","toolName":"mcp__a__b"}])
        );
    }

    #[test]
    fn drops_references_to_tools_missing_from_the_request_catalog() {
        let mut messages = vec![
            json!({"role":"tool","content":[{"type":"tool_reference","toolName":"mcp__gone__read"}]}),
            json!({"role":"tool","content":[{"type":"text","text":"ok"},{"type":"tool_reference","toolName":"mcp__gone__write"}]}),
        ];
        project(&mut messages, true, &available(&["mcp__still__read"]));
        assert_eq!(
            messages[0]["content"],
            json!([{
                "type": "text",
                "text": "[Tool references removed - tools no longer available]",
            }])
        );
        assert_eq!(messages[1]["content"], json!([{"type":"text","text":"ok"}]));
    }
}
