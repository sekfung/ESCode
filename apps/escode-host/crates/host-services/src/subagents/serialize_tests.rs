//! markdown 序列化（`rust-u2-subagents.md` §7）：行序、引号规则、model 编码、对象数组。
use super::*;
use serde_json::json;

fn config(value: serde_json::Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

#[test]
fn minimal_config_emits_name_description_and_body() {
    let markdown = serialize_subagent_markdown(&config(json!({
        "name": "demo",
        "description": "演示",
        "systemPrompt": "你是 demo。"
    })));
    assert_eq!(
        markdown,
        "---\nname: \"demo\"\ndescription: \"演示\"\n---\n\n你是 demo。\n"
    );
    // 空正文只留一个换行。
    let markdown = serialize_subagent_markdown(&config(json!({
        "name": "d", "description": "d", "systemPrompt": "   "
    })));
    assert_eq!(markdown, "---\nname: \"d\"\ndescription: \"d\"\n---\n\n");
}

#[test]
fn full_config_keeps_the_ts_line_order() {
    let markdown = serialize_subagent_markdown(&config(json!({
        "name": "demo",
        "description": "演示",
        "systemPrompt": "正文",
        "color": "blue",
        "modelSelection": {"providerId": "glm", "modelId": "glm-5", "options": {"reasoningLevel": "high"}},
        "tools": ["Read", "Grep(glob)"],
        "disallowedTools": ["Write"],
        "skills": ["pdf"],
        "permissionMode": "plan",
        "maxTurns": 12,
        "background": true,
        "injectAgentsMd": false,
        "mcpServers": ["server-a"]
    })));
    assert_eq!(
        markdown,
        "---\n\
name: \"demo\"\n\
description: \"演示\"\n\
color: blue\n\
model: glm/glm-5\n\
thoughtLevel: high\n\
tools:\n  - Read\n  - Grep(glob)\n\
disallowedTools:\n  - Write\n\
skills:\n  - pdf\n\
permissionMode: plan\n\
maxTurns: 12\n\
background: true\n\
injectAgentsMd: false\n\
mcpServers:\n  - server-a\n\
---\n\n正文\n"
    );
}

/// 不安全标量要加引号（`*`、含 `: `、看起来像数字/布尔/空值）。
#[test]
fn unsafe_scalars_are_quoted() {
    let markdown = serialize_subagent_markdown(&config(json!({
        "name": "d", "description": "d", "systemPrompt": "p",
        "tools": ["*", "a: b", "true", "42", "has#hash"]
    })));
    let tools = markdown
        .lines()
        .skip_while(|line| !line.starts_with("tools:"))
        .take(6)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(tools.contains("  - \"*\""));
    assert!(tools.contains("  - \"a: b\""));
    assert!(tools.contains("  - \"true\""));
    assert!(tools.contains("  - \"42\""));
    assert!(tools.contains("  - \"has#hash\""));
}

#[test]
fn model_values_use_custom_encoding_only_when_needed() {
    let model_of = |selection: serde_json::Value| {
        let markdown = serialize_subagent_markdown(&config(json!({
            "name": "d", "description": "d", "systemPrompt": "p",
            "modelSelection": selection
        })));
        markdown
            .lines()
            .find(|line| line.starts_with("model:"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        model_of(json!({"providerId": "glm", "modelId": "glm-5"})),
        "model: glm/glm-5"
    );
    // provider 含 `/` → 走编码。
    assert_eq!(
        model_of(json!({"providerId": "my/provider", "modelId": "m"})),
        "model: \"custom:my%2Fprovider:m\"",
        "TS 的 plain 判定也不允许 % 与 :"
    );
    // model 含 `$`（档位分隔符）→ 走编码。
    assert_eq!(
        model_of(json!({"providerId": "p", "modelId": "a$b"})),
        "model: \"custom:p:a%24b\""
    );
    // `custom:` 前缀本身就编码。
    assert_eq!(
        model_of(json!({"providerId": "custom:x", "modelId": "y"})),
        "model: \"custom:custom%3Ax:y\""
    );
}

/// 非字符串 `mcpServers` 按结构写回（块状 emitter），不能被清空。
#[test]
fn object_mcp_servers_are_written_structurally() {
    let markdown = serialize_subagent_markdown(&config(json!({
        "name": "d", "description": "d", "systemPrompt": "p",
        "mcpServers": [{"command": "node", "args": ["--foo", "--bar"]}]
    })));
    assert_eq!(
        markdown,
        "---\n\
name: \"d\"\n\
description: \"d\"\n\
mcpServers:\n  -\n    args:\n      - --foo\n      - --bar\n    command: node\n\
---\n\np\n"
    );
}
