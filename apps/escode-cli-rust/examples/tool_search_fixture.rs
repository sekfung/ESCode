//! Test-only ToolSearch driver：按行读 `{query,maxResults,catalog}`，输出 `{matches}`。
//! 用于 `packages/services/tests/escode-cli-rust-tool-search.test.ts` 与 Node oracle 做实时差分。
use escode_cli_domain::tool_search_catalog::{McpPresentation, ToolSearchCandidate, search_tools};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn candidate(value: &Value) -> ToolSearchCandidate {
    ToolSearchCandidate {
        name: value["name"].as_str().unwrap_or_default().to_string(),
        aliases: value["aliases"]
            .as_array()
            .map(|aliases| {
                aliases
                    .iter()
                    .filter_map(|alias| alias.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        description: value["description"].as_str().map(str::to_owned),
        search_hint: value["searchHint"].as_str().map(str::to_owned),
        mcp: value["mcp"].as_object().map(|mcp| McpPresentation {
            server_name: mcp["serverName"].as_str().unwrap_or_default().to_string(),
            tool_name: mcp["toolName"].as_str().unwrap_or_default().to_string(),
        }),
        deferred: value["deferred"].as_bool().unwrap_or(false),
    }
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin line");
        if line.trim().is_empty() {
            continue;
        }
        let input: Value = serde_json::from_str(&line).expect("json input");
        let catalog: Vec<ToolSearchCandidate> = input["catalog"]
            .as_array()
            .map(|entries| entries.iter().map(candidate).collect())
            .unwrap_or_default();
        let query = input["query"].as_str().unwrap_or_default();
        let max_results = input["maxResults"].as_u64().unwrap_or(5) as usize;
        let matches = search_tools(query, &catalog, max_results);
        writeln!(stdout, "{}", json!({ "matches": matches })).expect("stdout");
        stdout.flush().expect("flush");
    }
}
