//! ToolSearch 查询：tokenization、别名解析与相关度排序。
//! 目录定义与搜索实现分文件，避免单文件超过架构行数上限。

use regex::Regex;
use std::collections::HashSet;

use super::tool_search_catalog::{
    ToolSearchCandidate, parse_mcp_name, to_model_visible_mcp_name_part,
};

struct Tokenized {
    coarse: Vec<String>,
    parts: Vec<String>,
    full: String,
    mcp: bool,
}

fn tokenize(tool: &ToolSearchCandidate) -> Tokenized {
    let name = tool.name.as_str();
    let mcp_parts = tool
        .mcp
        .as_ref()
        .map(|m| (m.server_name.clone(), m.tool_name.clone()))
        .or_else(|| parse_mcp_name(name).map(|m| (m.server_name, m.tool_name)));
    if let Some((server, tool_name)) = mcp_parts {
        let coarse = vec![server.to_ascii_lowercase(), tool_name.to_ascii_lowercase()];
        let parts: Vec<String> = coarse
            .iter()
            .flat_map(|s| {
                s.split([' ', '_', '.'])
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        let full = parts.join(" ");
        return Tokenized {
            coarse,
            parts,
            full,
            mcp: true,
        };
    }
    let coarse = vec![name.to_ascii_lowercase()];
    let mut spaced = String::with_capacity(name.len() + 8);
    let mut previous_lower = false;
    for c in name.chars() {
        if previous_lower && c.is_ascii_uppercase() {
            spaced.push(' ');
        }
        previous_lower = c.is_ascii_lowercase();
        spaced.push(c);
    }
    let parts: Vec<String> = spaced
        .replace('_', " ")
        .to_ascii_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let full = parts.join(" ");
    Tokenized {
        coarse,
        parts,
        full,
        mcp: false,
    }
}

fn resolve_name<'a>(
    name: &str,
    catalog: &'a [ToolSearchCandidate],
) -> Option<&'a ToolSearchCandidate> {
    catalog
        .iter()
        .find(|tool| tool.name == name || tool.aliases.iter().any(|alias| alias == name))
}

fn escape_regex(term: &str) -> String {
    term.chars()
        .flat_map(|c| {
            if "\\.+*?()|[]{}^$".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

/// 上游 `searchTools` 的等价实现。返回按相关度降序的工具名。
pub fn search_tools(
    query: &str,
    catalog: &[ToolSearchCandidate],
    max_results: usize,
) -> Vec<String> {
    let max_results = max_results.max(1);
    let deferred: Vec<&ToolSearchCandidate> = catalog.iter().filter(|tool| tool.deferred).collect();

    if let Some(select) = query
        .strip_prefix("select:")
        .or_else(|| query.strip_prefix("SELECT:"))
    {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for value in select.split(',') {
            let name = value.trim();
            if name.is_empty() {
                continue;
            }
            if let Some(exact) = deferred
                .iter()
                .copied()
                .find(|tool| tool.name == name || tool.aliases.iter().any(|alias| alias == name))
                .or_else(|| resolve_name(name, catalog))
            {
                if seen.insert(exact.name.clone()) {
                    names.push(exact.name.clone());
                }
                continue;
            }
            for tool in catalog {
                let Some(mcp) = tool.mcp.as_ref() else {
                    continue;
                };
                let visible = to_model_visible_mcp_name_part(&mcp.tool_name);
                if (mcp.tool_name == name || visible == name) && seen.insert(tool.name.clone()) {
                    names.push(tool.name.clone());
                }
            }
        }
        return names;
    }

    let text = query.to_ascii_lowercase();
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if let Some(exact) = deferred
        .iter()
        .copied()
        .find(|tool| tool.name.to_ascii_lowercase() == text)
        .or_else(|| {
            catalog
                .iter()
                .find(|tool| tool.name.to_ascii_lowercase() == text)
        })
    {
        return vec![exact.name.clone()];
    }
    if text.starts_with("mcp__") && text.len() > 5 {
        let names: Vec<String> = deferred
            .iter()
            .filter(|tool| tool.name.to_ascii_lowercase().starts_with(text))
            .take(max_results)
            .map(|tool| tool.name.clone())
            .collect();
        if !names.is_empty() {
            return names;
        }
    }

    let terms: Vec<&str> = text
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect();
    let required: Vec<&str> = terms
        .iter()
        .filter(|term| term.starts_with('+') && term.len() > 1)
        .map(|term| &term[1..])
        .collect();
    let keywords: Vec<&str> = if required.is_empty() {
        terms.clone()
    } else {
        required
            .iter()
            .copied()
            .chain(
                terms
                    .iter()
                    .copied()
                    .filter(|term| !(term.starts_with('+') && term.len() > 1)),
            )
            .collect()
    };
    if keywords.is_empty() {
        return Vec::new();
    }
    let patterns: std::collections::HashMap<&str, Regex> = keywords
        .iter()
        .map(|term| {
            let pattern = Regex::new(&format!(r"\b{}\b", escape_regex(term)))
                .expect("escaped keyword compiles");
            (*term, pattern)
        })
        .collect();

    let mut scored: Vec<(String, i64)> = deferred
        .iter()
        .map(|tool| {
            let name = tokenize(tool);
            let description = tool
                .description
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let hint = tool
                .search_hint
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if !required.iter().all(|term| {
                name.parts.iter().any(|part| part.contains(term))
                    || name.coarse.iter().any(|part| part.contains(term))
                    || patterns
                        .get(term)
                        .is_some_and(|pattern| pattern.is_match(&description))
                    || patterns
                        .get(term)
                        .is_some_and(|pattern| pattern.is_match(&hint))
            }) {
                return (tool.name.clone(), 0);
            }
            let mut score = 0i64;
            for term in &keywords {
                if name.parts.iter().any(|part| part == term) {
                    score += if name.mcp { 12 } else { 10 };
                } else if name.parts.iter().any(|part| part.contains(term)) {
                    score += if name.mcp { 6 } else { 5 };
                }
                if name.coarse.iter().any(|part| part == term) {
                    score += if name.mcp { 12 } else { 10 };
                } else if name.coarse.iter().any(|part| part.contains(term)) {
                    score += if name.mcp { 4 } else { 3 };
                }
                if name.full.contains(term) && score == 0 {
                    score += 3;
                }
                if !hint.is_empty()
                    && patterns
                        .get(term)
                        .is_some_and(|pattern| pattern.is_match(&hint))
                {
                    score += 4;
                }
                if patterns
                    .get(term)
                    .is_some_and(|pattern| pattern.is_match(&description))
                {
                    score += 2;
                }
            }
            (tool.name.clone(), score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    scored
        .into_iter()
        .take(max_results)
        .map(|(name, _)| name)
        .collect()
}
