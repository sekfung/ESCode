//! frontmatter 切分与宽松解析（`rust-u2-subagents.md` §6）。
use super::*;

#[test]
fn split_requires_both_delimiters_and_strips_bom() {
    let split = split_markdown_frontmatter("---\nname: a\n---\n正文");
    assert_eq!(split.frontmatter.as_deref(), Some("name: a"));
    assert_eq!(split.body, "正文");
    // 缺第二个分隔符 / 首行不是 --- → 整段都是 body。
    for content in ["---\nname: a\n正文", "text\n---\nbody"] {
        let split = split_markdown_frontmatter(content);
        assert!(split.frontmatter.is_none(), "{content}");
    }
    // BOM 被去掉后再判。
    let split = split_markdown_frontmatter("\u{feff}---\na: 1\n---\nbody");
    assert_eq!(split.frontmatter.as_deref(), Some("a: 1"));
    // CRLF 也能切。
    let split = split_markdown_frontmatter("---\r\na: 1\r\n---\r\nbody");
    assert_eq!(split.frontmatter.as_deref(), Some("a: 1"));
}

#[test]
fn scalars_lists_and_inline_comments() {
    let parsed = parse_agent_frontmatter(
        "name: demo\nflag: true\ncount: 42\nlist:\n  - a\n  - b\ntags: [x, \"y z\"]\nnote: value # 注释\nquoted: \"a # b\"",
    );
    assert_eq!(parsed.values["name"], serde_json::json!("demo"));
    assert_eq!(parsed.values["flag"], serde_json::json!(true));
    assert_eq!(parsed.values["count"], serde_json::json!(42));
    assert_eq!(parsed.values["list"], serde_json::json!(["a", "b"]));
    assert_eq!(parsed.values["tags"], serde_json::json!(["x", "y z"]));
    assert_eq!(
        parsed.values["note"],
        serde_json::json!("value"),
        "行内注释被剥掉"
    );
    assert_eq!(
        parsed.values["quoted"],
        serde_json::json!("a # b"),
        "引号内的 # 不是注释"
    );
}

/// `mcpServers` 只接受「父服务器名字列表」：裸值、块状 mapping、未加引号的 `{...}` 项都判非法。
#[test]
fn mcp_servers_rejects_non_name_shapes() {
    let ok = parse_agent_frontmatter("mcpServers:\n  - server-a\n  - \"server-b\"");
    assert_eq!(
        ok.mcp_servers,
        Some(Some(vec!["server-a".to_owned(), "server-b".to_owned()]))
    );
    // 未配置 → 外层 None。
    assert_eq!(parse_agent_frontmatter("name: x").mcp_servers, None);
    // 裸值（`mcpServers:` 后没有列表项）→ Some(None)。
    assert_eq!(
        parse_agent_frontmatter("mcpServers:\nother: 1").mcp_servers,
        Some(None)
    );
    // 块状 mapping 续行 → Some(None)。
    assert_eq!(
        parse_agent_frontmatter("mcpServers:\n  command: node\ntools: x").mcp_servers,
        Some(None)
    );
    // 未加引号的 inline mapping 项 → Some(None)。
    assert_eq!(
        parse_agent_frontmatter("mcpServers: [{command: node}]").mcp_servers,
        Some(None)
    );
    // 加引号的字符串项合法。
    assert_eq!(
        parse_agent_frontmatter("mcpServers: [\"{command: node}\"]").mcp_servers,
        Some(Some(vec!["{command: node}".to_owned()]))
    );
}

#[test]
fn bare_tools_key_is_dropped() {
    let parsed = parse_agent_frontmatter("tools:\nname: x");
    assert!(
        !parsed.values.contains_key("tools"),
        "空值且没有列表项的 tools 整键丢掉"
    );
}
