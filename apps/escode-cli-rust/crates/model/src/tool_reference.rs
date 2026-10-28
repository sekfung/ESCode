//! ToolSearch 引用的跨模式投影（docs/specs/rust-tool-search.md）。
//!
//! `tool_reference` 只在 Anthropic native 模式的工具结果里有线上意义。会话从 native 切到
//! local/off 或另一家 provider 后，历史里仍可能保留引用块；发送前必须降级为文本，
//! 否则 provider 会因未知 part 拒绝整条请求（上游 `projectToolReferences` 的同一职责）。

use serde_json::{Value, json};

/// 把非 native 请求里的 `tool_reference` part 降级为文本；native 请求原样保留。
pub(super) fn project(messages: &mut [Value], native_tool_search: bool) {
    if native_tool_search {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downgrades_references_when_native_is_off() {
        let mut messages = vec![json!({
            "role": "tool",
            "content": [{"type":"tool_reference","toolName":"mcp__a__b"}]
        })];
        project(&mut messages, false);
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
        project(&mut messages, true);
        assert_eq!(
            messages[0]["content"],
            json!([{"type":"tool_reference","toolName":"mcp__a__b"}])
        );
    }
}
