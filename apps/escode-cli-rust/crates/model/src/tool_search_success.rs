//! native ToolSearch 成功提示（上游 `appendToolSearchSuccessMessages`）。
//!
//! 成功命中且结果仍以 `tool_reference` 回给 provider（native）时，请求投影在每段连续 tool
//! 结果之后追加一条 `role=user` 的 `"Tool loaded."`。提示只存在于请求，不写入历史
//! （docs/specs/rust-tool-search.md §5）。

use serde_json::{Value, json};

const TOOL_SEARCH_NAME: &str = "ToolSearch";
const LOADED_MESSAGE: &str = "Tool loaded.";

/// 在每段连续 tool 结果后按成功 ToolSearch 条数追加 `"Tool loaded."`。
pub(super) fn append(messages: &mut Vec<Value>) {
    if !messages.iter().any(is_loaded_search) {
        return;
    }
    let mut result = Vec::with_capacity(messages.len());
    let mut pending = 0;
    for (index, message) in messages.iter().enumerate() {
        if is_loaded_search(message) {
            pending += 1;
        }
        result.push(message.clone());
        // 整批 tool 结果必须连续；提示只在批次末尾派生，不能拆开 tool 结果。
        if messages.get(index + 1).is_some_and(|next| next["role"] == "tool") {
            continue;
        }
        for _ in 0..pending {
            result.push(json!({"role": "user", "content": LOADED_MESSAGE}));
        }
        pending = 0;
    }
    *messages = result;
}

fn is_loaded_search(message: &Value) -> bool {
    message["role"] == "tool"
        && message["_escode_tool_failed"] != true
        && message["_escode_tool_name"] == TOOL_SEARCH_NAME
        && message["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool_reference"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(id: &str) -> Value {
        json!({
            "role": "tool",
            "tool_call_id": id,
            "_escode_tool_name": TOOL_SEARCH_NAME,
            "content": [{"type": "tool_reference", "toolName": "mcp__demo__read"}],
        })
    }

    #[test]
    fn appends_one_loaded_message_per_successful_search_after_the_tool_run() {
        let mut messages = vec![
            json!({"role": "assistant", "content": "search"}),
            search("search-1"),
            json!({"role": "tool", "tool_call_id": "other", "content": "ok"}),
            search("search-2"),
            json!({"role": "assistant", "content": "done"}),
        ];
        append(&mut messages);
        assert_eq!(
            messages[4..6],
            [
                json!({"role": "user", "content": LOADED_MESSAGE}),
                json!({"role": "user", "content": LOADED_MESSAGE}),
            ]
        );
        assert_eq!(messages[6]["role"], "assistant");
    }

    #[test]
    fn appends_at_the_end_of_history() {
        let mut messages = vec![search("search-1")];
        append(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1], json!({"role": "user", "content": LOADED_MESSAGE}));
    }

    #[test]
    fn ignores_failed_text_and_reference_free_results() {
        let mut messages = vec![
            json!({"role": "tool", "_escode_tool_name": TOOL_SEARCH_NAME, "_escode_tool_failed": true, "content": [{"type": "tool_reference", "toolName": "x"}]}),
            json!({"role": "tool", "_escode_tool_name": TOOL_SEARCH_NAME, "content": [{"type": "text", "text": "<functions></functions>"}]}),
            json!({"role": "tool", "_escode_tool_name": TOOL_SEARCH_NAME, "content": "No matching deferred tools found"}),
            json!({"role": "tool", "_escode_tool_name": "DeferredToolCall", "content": [{"type": "tool_reference", "toolName": "x"}]}),
        ];
        append(&mut messages);
        assert_eq!(messages.len(), 4);
    }
}
