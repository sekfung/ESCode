//! Settings 侧摘要解析（`rust-u2-subagents.md` §6.5）。
use super::*;

fn parse(content: &str, scope: SummaryScope) -> SummaryOutcome {
    parse_subagent_markdown(content, "/agents/demo.md", scope)
}

/// 合并顺序是 `{...yaml, ...loose}`——**宽松覆盖 YAML**。所以块标量会被宽松解析改写成 `>-`
/// （TS 的真实行为，不是我们的偏差）；YAML 只在宽松读不出数组时兜底（见下一个用例）。
#[test]
fn loose_values_override_yaml_and_yaml_fills_the_gaps() {
    let content = "---\nname: demo\ndescription: >-\n  多行\n  描述\nmodel: glm/glm-5\n---\n正文";
    let agent = parse(content, SummaryScope::User)
        .agent
        .expect("应解析出摘要");
    assert_eq!(agent["description"], json!(">-"), "宽松解析覆盖了块标量");
    assert_eq!(agent["id"], json!("user:user:demo"));
    assert_eq!(agent["systemPrompt"], json!("正文"));
    assert_eq!(
        agent["modelSelection"],
        json!({"providerId": "glm", "modelId": "glm-5"})
    );
}

#[test]
fn mcp_servers_prefers_the_yaml_array_and_tools_keep_their_arguments() {
    let content = "---\nname: d\ndescription: x\ntools: [\"Read(glob)\", \" G r e p \"]\nmcpServers:\n  - a\n  - b\n---\nbody";
    let agent = parse(content, SummaryScope::User).agent.unwrap();
    // Settings 侧**不剥** `(...)`（runtime 侧会剥）。
    assert_eq!(agent["tools"], json!(["Read(glob)", "G r e p"]));
    assert_eq!(agent["mcpServers"], json!(["a", "b"]));
}

#[test]
fn missing_frontmatter_and_required_fields_match_ts_wording() {
    let outcome = parse("没有 frontmatter", SummaryScope::User);
    assert!(outcome.agent.is_none());
    let diagnostic = outcome.diagnostic.unwrap();
    assert_eq!(diagnostic["code"], json!("agent_missing_frontmatter"));
    assert_eq!(
        diagnostic["message"],
        json!("Agent Markdown must include frontmatter: /agents/demo.md")
    );
    let diagnostic = parse("---\ndescription: d\n---\nb", SummaryScope::User)
        .diagnostic
        .unwrap();
    assert_eq!(
        diagnostic["message"],
        json!("Agent frontmatter must include name: /agents/demo.md")
    );
}

#[test]
fn scope_and_source_map_like_ts() {
    let built_in = parse(
        "---\nname: x\ndescription: d\n---\nb",
        SummaryScope::BuiltIn,
    )
    .agent
    .unwrap();
    assert_eq!(built_in["source"], json!("built-in"));
    assert_eq!(built_in["scope"], json!("built-in"));
    assert_eq!(built_in["readOnly"], json!(true));
    assert_eq!(built_in["id"], json!("built-in:built-in:x"));

    // 工作区来源的 `source` 仍是 user（TS 同），但 scope 是 workspace 且可编辑。
    let workspace = parse(
        "---\nname: y\ndescription: d\n---\nb",
        SummaryScope::Workspace,
    )
    .agent
    .unwrap();
    assert_eq!(workspace["source"], json!("user"));
    assert_eq!(workspace["scope"], json!("workspace"));
    assert_eq!(workspace["readOnly"], json!(false));
    // settings 侧对项目级**不**丢 permissionMode（runtime 侧会丢）。
    let project = parse(
        "---\nname: z\ndescription: d\npermissionMode: plan\n---\nb",
        SummaryScope::Workspace,
    )
    .agent
    .unwrap();
    assert_eq!(project["permissionMode"], json!("plan"));
}
