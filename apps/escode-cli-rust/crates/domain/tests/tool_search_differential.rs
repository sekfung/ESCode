//! ToolSearch 搜索结果与上游 TS oracle 的差分语料
//! （语料由上游 `tool/search/search.ts` 生成，见 fixtures/tool_search_cases.json 的 source 字段）。

use escode_cli_domain::tool_search_catalog::{McpPresentation, ToolSearchCandidate, search_tools};
use serde_json::Value;

fn candidate(value: &Value) -> ToolSearchCandidate {
    ToolSearchCandidate {
        name: value["name"].as_str().expect("name").to_string(),
        aliases: value["aliases"]
            .as_array()
            .map(|aliases| {
                aliases
                    .iter()
                    .map(|alias| alias.as_str().expect("alias").to_string())
                    .collect()
            })
            .unwrap_or_default(),
        description: value["description"].as_str().map(str::to_owned),
        search_hint: value["searchHint"].as_str().map(str::to_owned),
        mcp: value["mcp"].as_object().map(|mcp| McpPresentation {
            server_name: mcp["serverName"].as_str().expect("serverName").to_string(),
            tool_name: mcp["toolName"].as_str().expect("toolName").to_string(),
        }),
        deferred: value["deferred"].as_bool().unwrap_or(false),
    }
}

fn run_fixture(raw: &str) {
    let fixture: Value = serde_json::from_str(raw).expect("fixture");
    let catalog: Vec<ToolSearchCandidate> = fixture["catalog"]
        .as_array()
        .expect("catalog")
        .iter()
        .map(candidate)
        .collect();
    let cases = fixture["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let query = case["query"].as_str().expect("query");
        let max_results = case["maxResults"].as_u64().expect("maxResults") as usize;
        let expected: Vec<String> = case["expected"]
            .as_array()
            .expect("expected")
            .iter()
            .map(|name| name.as_str().expect("name").to_string())
            .collect();
        assert_eq!(
            search_tools(query, &catalog, max_results),
            expected,
            "query={query} max_results={max_results}"
        );
    }
}

#[test]
fn matches_upstream_search_oracle() {
    run_fixture(include_str!("fixtures/tool_search_cases.json"));
}

/// 真实工具名/描述（来自 Rust `tool_surface.json`）的 Node oracle 语料：覆盖 cron/workflow/
/// MCP 等实际名称的 tokenization，而不只是手写样例。
#[test]
fn matches_upstream_search_oracle_with_real_tool_names() {
    run_fixture(include_str!("fixtures/tool_search_cases_realistic.json"));
}
