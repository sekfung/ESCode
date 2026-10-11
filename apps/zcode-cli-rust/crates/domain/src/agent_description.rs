//! Agent 工具的 provider 描述（TS `buildAgentProviderDescription`）与 agent 目录通告
//! （TS `collectAgentListingAttachment`）。文案来自生成资产（dynamicWorkflowEnabled=false）。
//! 见 docs/specs/rust-tool-surface.md 与 docs/specs/rust-v3.15.1-drift.md。

use std::sync::OnceLock;

use serde_json::Value;

use crate::subagent::Profile;

/// 子代理工具面强制剔除 plan 工具（TS `SUBAGENT_CHILD_FORCED_DISALLOWED_TOOLS`）。
const CHILD_FORCED_DISALLOWED: [&str; 2] = ["EnterPlanMode", "ExitPlanMode"];

/// 会话消息里 agent 目录通告的来源标记（TS `agent_listing_delta`）与增量载荷键。
pub const LISTING_SOURCE: &str = "agent_listing_delta";
pub const LISTING_DELTA_KEY: &str = "_zcode_agent_listing_delta";

fn template() -> &'static Value {
    static TEMPLATE: OnceLock<Value> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        serde_json::from_str(include_str!("agent_description_template.json"))
            .expect("generated agent description template")
    })
}

/// Agent 工具的静态 provider 描述（v3.15.1 起不再内嵌 profile 列表，目录改由
/// [`listing_delta`] 以 `<system-reminder>` 通告；两个搜索分支逐字相同）。
pub fn description() -> String {
    template()["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// TS `formatAgentProfileForPrompt`：`- name: description (Tools: …)`。
pub fn profile_line(profile: &Profile, embedded_search: bool) -> String {
    let template = template();
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let tools = if profile.name == "Explore" && profile.source == "built-in" {
        Some(text(
            &template["exploreTools"][if embedded_search {
                "embedded"
            } else {
                "direct"
            }],
        ))
    } else {
        profile
            .tools
            .as_ref()
            .map(|tools| child_tools(tools, &profile.disallowed_tools).join(", "))
    };
    match tools.filter(|tools| !tools.is_empty()) {
        Some(tools) => format!(
            "- {}: {} (Tools: {tools})",
            profile.name, profile.description
        ),
        None => format!("- {}: {}", profile.name, profile.description),
    }
}

/// TS `collectAgentListingAttachment`：按历史里已通告的名字（added 加、removed 减）算出本次增量，
/// 无变化回 `None`。返回 `(提醒正文, 增量载荷)`；载荷与 TS `AgentListingDelta` 同形，落进消息供后续
/// 请求与恢复读取。调用方只在本轮工具面含 Agent 时调用。
pub fn listing_delta(
    profiles: &[Profile],
    messages: &[Value],
    embedded_search: bool,
) -> Option<(String, Value)> {
    let mut announced: Vec<String> = Vec::new();
    for message in messages {
        if message["_zcode_source"] != LISTING_SOURCE {
            continue;
        }
        let delta = &message[LISTING_DELTA_KEY];
        for name in delta["addedTypes"].as_array().into_iter().flatten() {
            if let Some(name) = name.as_str()
                && !announced.iter().any(|known| known == name)
            {
                announced.push(name.to_owned());
            }
        }
        for name in delta["removedTypes"].as_array().into_iter().flatten() {
            announced.retain(|known| Some(known.as_str()) != name.as_str());
        }
    }
    let mut added: Vec<&Profile> = profiles
        .iter()
        .filter(|profile| !announced.iter().any(|name| *name == profile.name))
        .collect();
    let mut removed: Vec<String> = announced
        .iter()
        .filter(|name| !profiles.iter().any(|profile| profile.name == **name))
        .cloned()
        .collect();
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    added.sort_by(|left, right| locale_compare(&left.name, &right.name));
    // TS `removedTypes.sort()`：按 UTF-16 码元排序。
    removed.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    let is_initial = announced.is_empty();
    let added_lines: Vec<String> = added
        .iter()
        .map(|profile| profile_line(profile, embedded_search))
        .collect();
    let listing = &template()["listing"];
    let text = |key: &str| listing[key].as_str().unwrap_or_default().to_owned();
    let mut sections = Vec::new();
    if !added_lines.is_empty() {
        let heading = if is_initial {
            text("initialHeading")
        } else {
            text("addedHeading")
        };
        sections.push(format!("{heading}\n{}", added_lines.join("\n")));
    }
    if !removed.is_empty() {
        let lines: Vec<String> = removed.iter().map(|name| format!("- {name}")).collect();
        sections.push(format!("{}\n{}", text("removedHeading"), lines.join("\n")));
        sections.push(text("ambientNote"));
    }
    if is_initial {
        sections.push(text("concurrencyNote"));
    }
    let delta = serde_json::json!({
        "addedTypes": added.iter().map(|profile| profile.name.clone()).collect::<Vec<_>>(),
        "addedLines": added_lines,
        "removedTypes": removed,
        "isInitial": is_initial,
        "showConcurrencyNote": true,
    });
    Some((sections.join("\n\n"), delta))
}

/// 请求投影：规范历史里目录增量追加在本轮输入之后（只追加落库），TS 请求里它位于本轮输入正文之前；
/// 紧跟在一条真实用户输入（role=user、无 `_zcode_source`）之后的增量与之交换位置。工具往返之后的
/// 增量前面是 assistant / tool 消息，保持在末尾。
pub fn order_listing_before_input(messages: &mut [Value]) {
    for index in 1..messages.len() {
        let previous = &messages[index - 1];
        if messages[index]["_zcode_source"] == LISTING_SOURCE
            && previous["role"] == "user"
            && previous.get("_zcode_source").is_none()
        {
            messages.swap(index - 1, index);
        }
    }
}

/// TS `String.prototype.localeCompare`（ICU 根排序）在 profile 名字符集上的近似：一级忽略大小写、
/// 标点先于数字先于字母，三级小写在前，最后按码元。
fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    let primary = |c: char| {
        let lower = c.to_ascii_lowercase();
        match lower {
            'a'..='z' => 2_000 + lower as u32,
            '0'..='9' => 1_000 + lower as u32,
            other => other as u32,
        }
    };
    let key = |s: &str| s.chars().map(primary).collect::<Vec<_>>();
    let tertiary = |s: &str| {
        s.chars()
            .map(|c| c.is_ascii_uppercase())
            .collect::<Vec<_>>()
    };
    key(left)
        .cmp(&key(right))
        .then_with(|| tertiary(left).cmp(&tertiary(right)))
        .then_with(|| left.cmp(right))
}

/// TS `filterSubagentChildToolNames`：规则名取 `(` 之前，`web_search` 视为 WebSearch。
fn child_tools<'a>(tools: &'a [String], disallowed: &[String]) -> Vec<&'a str> {
    let rule_name = |rule: &str| {
        let trimmed = rule.trim();
        let name = match trimmed.find('(') {
            Some(index) if index > 0 => &trimmed[..index],
            _ => trimmed,
        };
        alias(name).to_owned()
    };
    let blocked: Vec<String> = CHILD_FORCED_DISALLOWED
        .iter()
        .map(|name| (*name).to_owned())
        .chain(disallowed.iter().map(|rule| rule_name(rule)))
        .filter(|name| !name.is_empty())
        .collect();
    tools
        .iter()
        .map(String::as_str)
        .filter(|tool| !blocked.iter().any(|name| name == alias(tool)))
        .collect()
}

fn alias(name: &str) -> &str {
    if name == "web_search" {
        "WebSearch"
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, source: &str, tools: Option<&[&str]>, disallowed: &[&str]) -> Profile {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "description": format!("{name} agent"),
            "source": source,
            "systemPrompt": "",
            "tools": tools,
            "disallowedTools": disallowed,
        }))
        .unwrap()
    }

    #[test]
    fn renders_profiles_like_ts() {
        let line = |name, source, tools, disallowed| {
            profile_line(&profile(name, source, tools, disallowed), true)
        };
        assert_eq!(
            line("general-purpose", "built-in", Some(&["*"][..]), &[][..]),
            "- general-purpose: general-purpose agent (Tools: *)"
        );
        assert_eq!(
            line("Explore", "built-in", Some(&["Bash", "Glob"][..]), &[][..]),
            "- Explore: Explore agent (Tools: Read, Bash, WebFetch, WebSearch, TodoWrite)"
        );
        assert_eq!(
            line(
                "custom",
                "user",
                Some(&["Read", "Bash", "EnterPlanMode", "web_search"][..]),
                &["Bash(rm:*)", "WebSearch"][..],
            ),
            "- custom: custom agent (Tools: Read)"
        );
        assert_eq!(line("bare", "project", None, &[][..]), "- bare: bare agent");
        assert!(!description().contains("- general-purpose"));
        assert!(description().contains("<system-reminder>"));
    }

    #[test]
    fn listing_moves_before_the_input_it_was_announced_for() {
        let listing = serde_json::json!({"role":"user","content":"l","_zcode_source":LISTING_SOURCE});
        let mut first = vec![
            serde_json::json!({"role":"user","content":"hi"}),
            listing.clone(),
        ];
        order_listing_before_input(&mut first);
        assert_eq!(first[0], listing);
        let mut later = vec![
            serde_json::json!({"role":"user","content":"hi"}),
            serde_json::json!({"role":"assistant","content":""}),
            listing.clone(),
        ];
        order_listing_before_input(&mut later);
        assert_eq!(later[2], listing);
    }

    #[test]
    fn listing_delta_announces_initial_added_and_removed_like_ts() {
        let general = profile("general-purpose", "built-in", Some(&["*"]), &[]);
        let custom = profile("custom", "user", None, &[]);
        let (initial, delta) =
            listing_delta(&[general.clone(), custom.clone()], &[], true).unwrap();
        // localeCompare：custom 排在 general-purpose 之前（忽略大小写）。
        assert_eq!(
            initial,
            "Available agent types for the Agent tool:\n- custom: custom agent\n- general-purpose: general-purpose agent (Tools: *)\n\nWhen you launch multiple agents for independent work, send them in a single message with multiple tool uses so they run concurrently."
        );
        let announced = serde_json::json!({
            "role": "user",
            "_zcode_source": LISTING_SOURCE,
            LISTING_DELTA_KEY: delta,
        });
        assert!(listing_delta(&[general.clone(), custom], &[announced.clone()], true).is_none());
        let extra = profile("Explore", "built-in", None, &[]);
        let (changed, _) = listing_delta(&[general, extra], &[announced], true).unwrap();
        assert_eq!(
            changed,
            "New agent types are now available for the Agent tool:\n- Explore: Explore agent (Tools: Read, Bash, WebFetch, WebSearch, TodoWrite)\n\nThe following agent types are no longer available:\n- custom\n\nThis is ambient context — do not narrate it to the user unless they ask or it is directly relevant to their request."
        );
    }
}
