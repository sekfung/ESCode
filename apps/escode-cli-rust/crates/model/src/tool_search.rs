//! Provider-native tool search 的线上编码（docs/specs/rust-tool-search.md）。
//! 会话侧在 native 模式给延迟工具加 `deferLoading`，Anthropic Messages 需要 beta 头；
//! 客户端仍只发送普通 function 定义，由 provider 展开 schema。

use serde_json::Value;

/// Anthropic advanced tool use beta（上游 `TOOL_SEARCH_BETA`）。
pub const ANTHROPIC_BETA: &str = "advanced-tool-use-2025-11-20";

/// 编码后的请求体是否包含 provider 原生延迟工具。
pub(super) fn needs_beta_body(body: &Value) -> bool {
    body["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["defer_loading"] == true))
}

/// 合并 web search 与 tool search 两个 beta，去重并保留调用方已有顺序。
pub(super) fn merge_beta(
    existing: Option<&str>,
    web_search: bool,
    tool_search: bool,
) -> Option<String> {
    let mut values: Vec<String> = existing
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if web_search
        && !values
            .iter()
            .any(|value| value == super::web_search::ANTHROPIC_BETA)
    {
        values.push(super::web_search::ANTHROPIC_BETA.to_owned());
    }
    if tool_search && !values.iter().any(|value| value == ANTHROPIC_BETA) {
        values.push(ANTHROPIC_BETA.to_owned());
    }
    (!values.is_empty()).then(|| values.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_deferred_tools_in_body() {
        assert!(needs_beta_body(&json!({"tools":[{"defer_loading":true}]})));
        assert!(!needs_beta_body(&json!({"tools":[{"name":"Read"}]})));
    }

    #[test]
    fn merges_betas_without_duplicates() {
        assert_eq!(merge_beta(None, false, true).unwrap(), ANTHROPIC_BETA);
        assert_eq!(
            merge_beta(Some("x-1"), true, true).unwrap(),
            format!(
                "x-1,{},{}",
                super::super::web_search::ANTHROPIC_BETA,
                ANTHROPIC_BETA
            )
        );
        assert_eq!(
            merge_beta(Some(ANTHROPIC_BETA), false, true).unwrap(),
            ANTHROPIC_BETA
        );
    }
}
