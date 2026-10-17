//! markdown → profile（`rust-u2-subagents.md` §6）。
use super::*;

fn parse(content: &str, source: ProfileSource) -> Result<ParsedProfile, Diagnostic> {
    parse_agent_profile_from_markdown(content, Some("/agents/demo.md"), source)
}

#[test]
fn full_frontmatter_maps_to_a_profile() {
    let content = "---\nname: demo\ndescription: 演示 agent\nmodel: glm/glm-5$high\ncolor: blue\npermissionMode: plan\nmaxTurns: 12\nmemory: project\ntools: Read, Grep(glob)\ndisallowedTools:\n  - Write\nskills: [pdf, docs]\nbackground: \"true\"\ninjectAgentsMd: false\nmcpServers: [server-a]\n---\n  你是 demo。\n";
    let parsed = parse(content, ProfileSource::User).unwrap();
    let profile = &parsed.profile;
    assert_eq!(profile["name"], serde_json::json!("demo"));
    assert_eq!(profile["description"], serde_json::json!("演示 agent"));
    assert_eq!(profile["source"], serde_json::json!("user"));
    assert_eq!(
        profile["systemPrompt"],
        serde_json::json!("你是 demo。"),
        "body 要 trim"
    );
    assert_eq!(profile["path"], serde_json::json!("/agents/demo.md"));
    assert_eq!(
        profile["modelSelection"],
        serde_json::json!({
            "providerId": "glm", "modelId": "glm-5", "options": {"reasoningLevel": "high"}
        })
    );
    assert_eq!(profile["color"], serde_json::json!("blue"));
    assert_eq!(profile["permissionMode"], serde_json::json!("plan"));
    assert_eq!(profile["maxTurns"], serde_json::json!(12));
    assert_eq!(profile["memory"], serde_json::json!("project"));
    assert_eq!(
        profile["tools"],
        serde_json::json!(["Read", "Grep"]),
        "括号参数被剥掉"
    );
    assert_eq!(profile["disallowedTools"], serde_json::json!(["Write"]));
    assert_eq!(profile["skills"], serde_json::json!(["pdf", "docs"]));
    assert_eq!(profile["background"], serde_json::json!(true));
    assert_eq!(profile["injectAgentsMd"], serde_json::json!(false));
    assert_eq!(profile["mcpServers"], serde_json::json!(["server-a"]));
    assert!(parsed.diagnostic.is_none());
}

#[test]
fn missing_required_fields_fail_with_ts_wording() {
    let missing_frontmatter = parse("没有 frontmatter", ProfileSource::User).unwrap_err();
    assert_eq!(missing_frontmatter.code, "agent_missing_frontmatter");
    assert_eq!(
        missing_frontmatter.message,
        "Agent Markdown must include frontmatter: /agents/demo.md"
    );
    let missing_name = parse("---\ndescription: d\n---\nbody", ProfileSource::User).unwrap_err();
    assert_eq!(missing_name.code, "agent_missing_required_frontmatter");
    assert_eq!(
        missing_name.message,
        "Agent frontmatter must include name: /agents/demo.md"
    );
    let missing_description = parse("---\nname: n\n---\nbody", ProfileSource::User).unwrap_err();
    assert_eq!(
        missing_description.message,
        "Agent frontmatter must include description: /agents/demo.md"
    );
    let invalid_mcp = parse(
        "---\nname: n\ndescription: d\nmcpServers:\n  command: x\n---\nbody",
        ProfileSource::User,
    )
    .unwrap_err();
    assert_eq!(invalid_mcp.code, "agent_invalid_mcp_servers");
}

/// 项目级来源不允许 `permissionMode`；`memory` 非法是带 profile 的**警告**。
#[test]
fn project_source_drops_permission_mode_and_memory_is_a_warning() {
    let content =
        "---\nname: n\ndescription: d\npermissionMode: bypassPermissions\nmemory: nope\n---\nbody";
    let parsed = parse(content, ProfileSource::Project).unwrap();
    assert!(
        parsed.profile.get("permissionMode").is_none(),
        "项目级不透传权限模式"
    );
    let diagnostic = parsed.diagnostic.expect("memory 非法要报诊断");
    assert_eq!(diagnostic.code, "agent_invalid_memory_scope");
    assert_eq!(
        diagnostic.message,
        "Agent frontmatter memory must be user, project, or local: /agents/demo.md"
    );
    // 非项目级保留权限模式。
    let parsed = parse(content, ProfileSource::User).unwrap();
    assert_eq!(
        parsed.profile["permissionMode"],
        serde_json::json!("bypassPermissions")
    );
}

#[test]
fn model_selection_handles_inherit_custom_and_picker_forms() {
    let with_model = |model: &str, level: Option<&str>| {
        let thought = level
            .map(|level| format!("thoughtLevel: {level}\n"))
            .unwrap_or_default();
        let content = format!("---\nname: n\ndescription: d\nmodel: {model}\n{thought}---\nbody");
        parse_agent_profile_from_markdown(&content, Some("/a.md"), ProfileSource::User)
            .ok()
            .and_then(|parsed| parsed.profile.get("modelSelection").cloned())
    };
    // 继承名与缺失都不产生覆盖。
    for model in ["inherit", "main", "sonnet", "opus", "haiku"] {
        assert_eq!(with_model(model, None), None, "{model}");
    }
    // 自定义编码（含百分号转义）。
    assert_eq!(
        with_model("custom:my%2Fprovider:model%20x", None),
        Some(serde_json::json!({"providerId": "my/provider", "modelId": "model x"}))
    );
    // `$` 在末位不当档位。
    assert_eq!(
        with_model("glm/glm-5$", None),
        Some(serde_json::json!({"providerId": "glm", "modelId": "glm-5$"}))
    );
    // thoughtLevel 覆盖到 options。
    assert_eq!(
        with_model("glm/glm-5", Some("low")),
        Some(serde_json::json!({
            "providerId": "glm", "modelId": "glm-5", "options": {"reasoningLevel": "low"}
        }))
    );
    // 缺 provider 段 → 无覆盖。
    assert_eq!(with_model("no-slash", None), None);
}

#[test]
fn tools_array_that_normalizes_empty_becomes_an_empty_list() {
    let content = "---\nname: n\ndescription: d\ntools:\n  - \" \"\n---\nbody";
    let parsed = parse(content, ProfileSource::User).unwrap();
    assert_eq!(
        parsed.profile["tools"],
        serde_json::json!([]),
        "tools 写成数组但解析为空时显式回 []"
    );
}
