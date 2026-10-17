//! agent markdown 的 frontmatter 切分与宽松解析（TS `subagent-profile-frontmatter.ts`，243 行）。
//!
//! 这是一个**手写的 YAML 子集**解析器（没有第三方 YAML 依赖），逐条复刻：
//! 空行/注释跳过、`- ` 列表项、`key:` 空值开启列表、缩进续行视为块状 mapping（对 `mcpServers`
//! 记为非法）、内联 `[...]`/`{...}`、行内注释（`#` 前须有空白）、引号成对才去引号。

use serde_json::{Map, Value};

use super::frontmatter_value::{
    has_unquoted_inline_mapping_item, is_unquoted_mapping_value, parse_mcp_server_list_item,
    parse_scalar_value,
};

pub struct SplitFrontmatter {
    pub body: String,
    pub frontmatter: Option<String>,
}

/// TS `splitMarkdownFrontmatter`：去 BOM；首行必须是 `---`；取紧随其后的第二个 `---`。
pub fn split_markdown_frontmatter(content: &str) -> SplitFrontmatter {
    let normalized = content.strip_prefix('\u{feff}').unwrap_or(content);
    if !normalized.starts_with("---") {
        return SplitFrontmatter {
            body: normalized.to_owned(),
            frontmatter: None,
        };
    }
    let lines: Vec<&str> = normalized
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    if lines.first().map(|line| line.trim()) != Some("---") {
        return SplitFrontmatter {
            body: normalized.to_owned(),
            frontmatter: None,
        };
    }
    let end = lines
        .iter()
        .enumerate()
        .position(|(index, line)| index > 0 && line.trim() == "---");
    let Some(end) = end else {
        return SplitFrontmatter {
            body: normalized.to_owned(),
            frontmatter: None,
        };
    };
    SplitFrontmatter {
        frontmatter: Some(lines[1..end].join("\n")),
        body: lines[end + 1..].join("\n"),
    }
}

/// TS `ParsedAgentFrontmatter`：外层 `None` = 未配置；`Some(None)` = 配置了但不合法。
pub struct ParsedAgentFrontmatter {
    pub mcp_servers: Option<Option<Vec<String>>>,
    pub values: Map<String, Value>,
}

struct LooseFrontmatter {
    bare_value_keys: Vec<String>,
    invalid_nested_list_keys: Vec<String>,
    values: Map<String, Value>,
}

/// TS `parseAgentFrontmatter`。
pub fn parse_agent_frontmatter(frontmatter: &str) -> ParsedAgentFrontmatter {
    let loose = parse_loose_frontmatter(frontmatter);
    let mut values = loose.values;
    // `tools` 若写成空值（后面没跟列表项）整键丢掉（TS `delete values.tools`）。
    if loose.bare_value_keys.iter().any(|key| key == "tools") {
        values.remove("tools");
    }
    let mcp_servers = parse_mcp_server_names(
        values.get("mcpServers"),
        values.contains_key("mcpServers"),
        loose.bare_value_keys.iter().any(|key| key == "mcpServers"),
        loose
            .invalid_nested_list_keys
            .iter()
            .any(|key| key == "mcpServers"),
    );
    ParsedAgentFrontmatter {
        mcp_servers,
        values,
    }
}

fn parse_mcp_server_names(
    value: Option<&Value>,
    configured: bool,
    has_bare_value: bool,
    has_invalid_nested: bool,
) -> Option<Option<Vec<String>>> {
    if !configured {
        return None;
    }
    let Some(items) = value.and_then(Value::as_array) else {
        return Some(None);
    };
    if has_bare_value || has_invalid_nested {
        return Some(None);
    }
    let mut names = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            return Some(None);
        };
        let name = text.trim();
        if name.is_empty() {
            return Some(None);
        }
        names.push(name.to_owned());
    }
    Some(Some(names))
}

fn parse_loose_frontmatter(frontmatter: &str) -> LooseFrontmatter {
    let mut bare_value_keys: Vec<String> = Vec::new();
    let mut invalid_nested_list_keys: Vec<String> = Vec::new();
    let mut values = Map::new();
    let mut pending_list_key: Option<String> = None;

    for raw_line in frontmatter.split('\n') {
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        if let Some(item) = list_item(line)
            && let Some(key) = pending_list_key.clone()
        {
            bare_value_keys.retain(|candidate| candidate != &key);
            if key == "mcpServers" && is_unquoted_mapping_value(&item) {
                invalid_nested_list_keys.push(key.clone());
            }
            let mut existing: Vec<Value> = values
                .get(&key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            existing.push(if key == "mcpServers" {
                parse_mcp_server_list_item(&item)
            } else {
                parse_scalar_value(&item, false)
            });
            values.insert(key, Value::Array(existing));
            continue;
        }

        if pending_list_key.is_some() && raw_line.starts_with([' ', '\t']) {
            // 块状 mapping 不是 server-name 列表：不能被空数组吞掉后扩大 child MCP scope。
            if let Some(key) = pending_list_key.take() {
                invalid_nested_list_keys.push(key);
            }
            continue;
        }

        pending_list_key = None;
        let Some((key, raw_value)) = key_value(line) else {
            continue;
        };
        if raw_value.trim().is_empty() {
            values.insert(key.clone(), Value::Array(Vec::new()));
            bare_value_keys.push(key.clone());
            pending_list_key = Some(key);
            continue;
        }
        if key == "mcpServers" && has_unquoted_inline_mapping_item(&raw_value) {
            invalid_nested_list_keys.push(key.clone());
        }
        values.insert(
            key.clone(),
            parse_scalar_value(&raw_value, key == "mcpServers"),
        );
    }

    LooseFrontmatter {
        bare_value_keys,
        invalid_nested_list_keys,
        values,
    }
}

/// `^\s*-\s+(.*)$`
fn list_item(line: &str) -> Option<String> {
    let trimmed_start = line.trim_start();
    let rest = trimmed_start.strip_prefix('-')?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let item = rest.trim_start();
    if item.is_empty() {
        return None;
    }
    Some(item.to_owned())
}

/// `^([A-Za-z][A-Za-z0-9_-]*)\s*:\s*(.*)$`
fn key_value(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' || bytes[index] == b'-')
    {
        index += 1;
    }
    let key = &line[..index];
    let rest = &line[index..];
    let rest = rest.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix(':')?;
    let value = rest.trim_start_matches([' ', '\t']);
    Some((key.to_owned(), value.to_owned()))
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;
