//! ToolSearch 目录增删/恢复公告（上游 `buildToolSearchAnnouncement` / `collectToolSearchHistory`）。
//! 文案与增量规则逐字对齐 `apps/zcode-cli/packages/core/src/tool/search/exposure.ts` 与
//! `history.ts`；契约见 docs/specs/rust-tool-search.md §5.1。

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::tool_search_catalog::ResolvedToolSearchMode;

/// 公告消息的来源标记（TS `tool_search_announcement`）与增量载荷键。
pub const ANNOUNCEMENT_SOURCE: &str = "tool_search_announcement";
pub const ANNOUNCEMENT_KEY: &str = "_escode_tool_search_announcement";

const TOOL_SEARCH_NAME: &str = "ToolSearch";
const MAX_REMOVED_NAME_LINES: usize = 30;
const ADDED_TOOLS_NOTICE: &str = "The following deferred tools are now available via ToolSearch. Their schemas are NOT loaded — calling them directly will fail with InputValidationError. Use ToolSearch with query \"select:<name>[,<name>...]\" to load tool schemas before calling them:";
const LOCAL_ADDED_TOOLS_NOTICE: &str = "The following deferred tools are available via ToolSearch. Use query \"select:<name>[,<name>...]\" to load their complete schemas, then call them through DeferredToolCall with {name, input}. Call resident tools directly:";
pub(crate) const AMBIENT_CONTEXT_NOTICE: &str = "This is ambient context — do not narrate it to the user unless they ask or it is directly relevant to their request.";

/// 从保留历史收集已发现（schema 已交付给模型）的工具名：tool_reference part 与 compact 边界。
/// 恢复后用它回填会话已发现集合，避免把已加载工具重新当作延迟工具或重复公告。
pub fn discovered(messages: &[Value]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for message in messages {
        // 上游只读 entry.metadata.compactBoundary；Rust 的 legacy 会话把边界放在消息顶层。
        for boundary in [&message["metadata"]["compactBoundary"], &message["compactBoundary"]] {
            for name in boundary["preCompactDiscoveredTools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                push_unique(&mut names, name);
            }
        }
        if message.get("_escode_source").is_some() {
            continue;
        }
        if message["role"] != "tool" || message["_escode_tool_failed"] == true {
            continue;
        }
        let Some(parts) = message["content"].as_array() else {
            continue;
        };
        for part in parts {
            if part["type"] == "tool_reference"
                && let Some(name) = part["toolName"].as_str()
            {
                push_unique(&mut names, name);
            }
        }
    }
    names
}

/// 按历史里的公告增量与本轮目录计算下一段公告；无变化回 `None`。
///
/// `deferred` 是本轮 ToolSearch catalog 中标记为延迟的工具名，`available` 是本轮注册目录的
/// 全部工具名（不可用判定必须对照完整作用域，deferred→alwaysLoad 仍算可用）。
pub fn build(
    messages: &[Value],
    deferred: &[String],
    available: &HashSet<String>,
    mode: ResolvedToolSearchMode,
) -> Option<Value> {
    if mode == ResolvedToolSearchMode::Off {
        return None;
    }
    let (announced, listed) = announced_sets(messages);
    let mut names: Vec<&String> = deferred.iter().collect();
    names.sort();
    names.dedup();
    let added_names: Vec<String> = names
        .iter()
        .filter(|name| !announced.contains(**name) || !listed.contains(**name))
        .map(|name| (*name).clone())
        .collect();
    let readded_names: Vec<String> = names
        .iter()
        .filter(|name| !announced.contains(**name) && listed.contains(**name))
        .map(|name| (*name).clone())
        .collect();
    let mut removed_names: Vec<String> = announced
        .iter()
        .filter(|name| !available.contains(*name))
        .cloned()
        .collect();
    removed_names.sort();
    if added_names.is_empty() && removed_names.is_empty() {
        return None;
    }

    let readded: HashSet<&str> = readded_names.iter().map(String::as_str).collect();
    let listed_names: Vec<&str> = added_names
        .iter()
        .map(String::as_str)
        .filter(|name| !readded.contains(name))
        .collect();
    let mut paragraphs: Vec<String> = Vec::new();
    if !listed_names.is_empty() {
        let notice = if mode == ResolvedToolSearchMode::Local {
            LOCAL_ADDED_TOOLS_NOTICE
        } else {
            ADDED_TOOLS_NOTICE
        };
        paragraphs.push(format!("{notice}\n{}", listed_names.join("\n")));
    }
    if !readded_names.is_empty() {
        paragraphs.push(format!(
            "{} deferred tool{} available again (names announced earlier in this conversation): {}. Load via {TOOL_SEARCH_NAME} as before.",
            readded_names.len(),
            if readded_names.len() == 1 { " is" } else { "s are" },
            summarize(&readded_names)
        ));
    }
    if !removed_names.is_empty() {
        if removed_names.len() > MAX_REMOVED_NAME_LINES {
            paragraphs.push(format!(
                "{} deferred tools are no longer available: {}. Do not search for them — {TOOL_SEARCH_NAME} will return no match.",
                removed_names.len(),
                summarize(&removed_names)
            ));
        } else {
            paragraphs.push(format!(
                "The following deferred tools are no longer available. Do not search for them — {TOOL_SEARCH_NAME} will return no match:\n{}",
                removed_names.join("\n")
            ));
        }
        paragraphs.push(AMBIENT_CONTEXT_NOTICE.to_owned());
    }

    let delta = json!({
        "addedNames": added_names,
        "removedNames": removed_names,
        "readdedNames": readded_names,
    });
    let mut message = Map::new();
    message.insert("role".into(), json!("user"));
    message.insert(
        "content".into(),
        json!(format!(
            "<system-reminder>\n{}\n</system-reminder>",
            paragraphs.join("\n\n")
        )),
    );
    message.insert("_escode_source".into(), json!(ANNOUNCEMENT_SOURCE));
    message.insert(ANNOUNCEMENT_KEY.into(), delta);
    Some(Value::Object(message))
}

fn announced_sets(messages: &[Value]) -> (HashSet<String>, HashSet<String>) {
    let mut announced: HashSet<String> = HashSet::new();
    let mut listed: HashSet<String> = HashSet::new();
    for message in messages {
        let delta = &message[ANNOUNCEMENT_KEY];
        if !delta.is_object() {
            continue;
        }
        let readded: HashSet<&str> = delta["readdedNames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for name in delta["addedNames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            announced.insert(name.to_owned());
            // 恢复提示不算完整列名：原始列名被压缩后仍需补齐。
            if !readded.contains(name) {
                listed.insert(name.to_owned());
            }
        }
        for name in delta["removedNames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            announced.remove(name);
        }
    }
    (announced, listed)
}

/// 请求投影：公告只追加落库，但上游 provider 视图把它 bubble 到本轮真实输入之前。
/// 向前跨过同一轮的其它 reminder；遇到工具/助手消息即停，保持公告在它所属的步骤之后。
pub fn order_before_input(messages: &mut Vec<Value>) {
    let mut index = messages.len();
    while index > 0 {
        index -= 1;
        if messages[index]["_escode_source"] != ANNOUNCEMENT_SOURCE {
            continue;
        }
        let mut at = index;
        while at > 0 {
            let previous = &messages[at - 1];
            if previous["role"] != "user" {
                break;
            }
            if previous.get("_escode_source").is_none() {
                let message = messages.remove(index);
                messages.insert(at - 1, message);
                break;
            }
            at -= 1;
        }
    }
}

pub(crate) fn summarize(names: &[String]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for name in names {
        let group = match name.strip_prefix("mcp__") {
            Some(rest) => format!(
                "mcp__{}__*",
                rest.split("__").next().unwrap_or(rest)
            ),
            None => name.clone(),
        };
        match counts.iter_mut().find(|(known, _)| *known == group) {
            Some((_, count)) => *count += 1,
            None => counts.push((group, 1)),
        }
    }
    counts.sort_by(|(a, _), (b, _)| locale_cmp(a, b));
    counts
        .iter()
        .map(|(group, count)| {
            if *count > 1 {
                format!("{group} ({count})")
            } else {
                group.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// 上游用 `localeCompare`：大小写不敏感优先，大小写敏感兜底。
fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

fn push_unique(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|known| known == name) {
        names.push(name.to_owned());
    }
}

