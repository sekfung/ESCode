//! Provider-native `web_search` 的线上编码（docs/specs/rust-websearch.md，对齐 AI SDK
//! `anthropic.web_search_20260209`）。会话侧以 `{"type":"_zcode_native_web_search",…}` 标记请求该工具。
use serde_json::{Value, json};

/// 会话侧使用的内部工具标记（不直接发给供应商）。
pub use crate::domain::web_search::MARKER;
/// AI SDK 为 web_search_20260209 附加的 beta。
pub const ANTHROPIC_BETA: &str = "code-execution-web-tools-2026-02-09";

pub(super) fn is_native(tool: &Value) -> bool {
    tool["type"] == MARKER
}

/// Anthropic 工具块；未提供的可选字段省略（AI SDK 以 `undefined` 序列化即省略）。
pub(super) fn anthropic_tool(tool: &Value) -> Option<Value> {
    if !is_native(tool) {
        return None;
    }
    let mut native = json!({"type":"web_search_20260209","name":"web_search","max_uses":tool["max_uses"]});
    for key in ["allowed_domains", "blocked_domains"] {
        if tool[key].as_array().is_some_and(|domains| !domains.is_empty()) {
            native[key] = tool[key].clone();
        }
    }
    Some(native)
}

/// 编码后的请求体是否需要 native 搜索 beta。
pub(super) fn needs_beta(body: &Value) -> bool {
    body["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|t| t["type"] == "web_search_20260209"))
}

/// 与配置中已有的 `anthropic-beta` 逗号合并（AI SDK 合并 user-supplied betas）。
pub(super) fn merge_beta(existing: Option<&str>) -> String {
    match existing.map(str::trim).filter(|v| !v.is_empty()) {
        Some(existing) if existing.split(',').any(|b| b.trim() == ANTHROPIC_BETA) => {
            existing.to_owned()
        }
        Some(existing) => format!("{existing},{ANTHROPIC_BETA}"),
        None => ANTHROPIC_BETA.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_the_ai_sdk() {
        let marker = json!({"type":MARKER,"max_uses":8,"allowed_domains":["a.example"],"blocked_domains":[]});
        assert_eq!(
            anthropic_tool(&marker).unwrap(),
            json!({"type":"web_search_20260209","name":"web_search","max_uses":8,"allowed_domains":["a.example"]})
        );
        assert!(anthropic_tool(&json!({"type":"function"})).is_none());
        assert!(needs_beta(&json!({"tools":[anthropic_tool(&marker).unwrap()]})));
        assert_eq!(merge_beta(None), ANTHROPIC_BETA);
        assert_eq!(merge_beta(Some("x-1")), format!("x-1,{ANTHROPIC_BETA}"));
        assert_eq!(merge_beta(Some(ANTHROPIC_BETA)), ANTHROPIC_BETA);
    }
}
