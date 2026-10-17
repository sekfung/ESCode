//! `detect`（`rust-u2-sync.md` §3.4）。
use super::*;

fn compare() -> Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn request(categories: &[&str]) -> Value {
    json!({"intent": "manualImport", "categories": categories})
}

#[test]
fn skills_report_target_and_name_conflicts() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/skills/alpha/SKILL.md"),
        "---\nname: alpha\nversion: \"1.0\"\n---\n",
    );
    write(
        &home.path().join(".claude/skills/beta/SKILL.md"),
        "---\nname: beta\n---\n",
    );
    std::fs::create_dir_all(home.path().join(".escode/skills/alpha")).unwrap();
    write(
        &home.path().join(".escode/skills/renamed/SKILL.md"),
        "---\nname: beta\n---\n",
    );

    let result = detect(home.path(), None, &request(&["skills"]), &compare());
    let agents = result["agents"].as_array().unwrap();
    let agent = &agents[0];
    assert_eq!(agent["agent"], json!("claudeCode"));
    let category = &agent["categories"][0];
    assert_eq!(category["category"], json!("skills"));
    assert_eq!(category["discoveredCount"], json!(2));
    assert_eq!(category["importableCount"], json!(0));
    assert_eq!(category["skippedCount"], json!(2));
    assert_eq!(category["selectedByDefault"], json!(false));
    let skills = category["sourceRoots"][0]["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], json!("alpha"));
    assert_eq!(skills[0]["skipReason"], json!("targetExists"));
    assert_eq!(skills[0]["version"], json!("1.0"));
    assert_eq!(skills[1]["name"], json!("beta"));
    assert_eq!(skills[1]["skipReason"], json!("sameNameExists"));
    assert_eq!(category["sourceRoots"][0]["scope"], json!("global"));
}

#[test]
fn skills_are_selected_by_default_when_importable() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/skills/gamma/SKILL.md"),
        "---\nname: gamma\n---\n",
    );
    let result = detect(home.path(), None, &request(&["skills"]), &compare());
    let category = &result["agents"][0]["categories"][0];
    assert_eq!(category["importableCount"], json!(1));
    assert_eq!(category["selectedByDefault"], json!(true));
}

#[test]
fn other_categories_report_their_agents() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".claude/commands/run.md"), "body");
    write(
        &home
            .path()
            .join(".claude/plugins/plug/.claude-plugin/plugin.json"),
        r#"{"name":"plug"}"#,
    );
    write(
        &home.path().join(".claude/settings.json"),
        r#"{"mcpServers":{"docs":{"command":"uvx"}}}"#,
    );

    let commands = detect(home.path(), None, &request(&["commands"]), &compare());
    let category = &commands["agents"][0]["categories"][0];
    assert_eq!(category["category"], json!("commands"));
    assert_eq!(category["importableCount"], json!(1));
    assert_eq!(category["selectedByDefault"], json!(false));
    assert_eq!(
        category["sourceRoots"][0]["commands"][0]["name"],
        json!("/run")
    );

    let plugins = detect(home.path(), None, &request(&["plugins"]), &compare());
    let category = &plugins["agents"][0]["categories"][0];
    assert_eq!(category["category"], json!("plugins"));
    assert_eq!(
        category["sourceRoots"][0]["plugins"][0]["name"],
        json!("plug")
    );

    let mcp = detect(home.path(), None, &request(&["mcpServers"]), &compare());
    let category = &mcp["agents"][0]["categories"][0];
    assert_eq!(category["category"], json!("mcpServers"));
    assert_eq!(
        category["sourceRoots"][0]["mcpServers"][0]["name"],
        json!("docs")
    );
}

#[test]
fn detect_requires_manual_import_and_a_supported_category() {
    let home = tempfile::tempdir().unwrap();
    let result = detect(
        home.path(),
        None,
        &json!({"categories": ["skills"]}),
        &compare(),
    );
    assert_eq!(result, json!({"agents": []}));
    let result = detect(
        home.path(),
        None,
        &json!({"intent": "manualImport", "categories": ["providers"]}),
        &compare(),
    );
    assert_eq!(result, json!({"agents": []}));
    // 多个类别时按 skills → commands → plugins → mcp 的优先级只扫描第一个。
    write(
        &home.path().join(".claude/skills/alpha/SKILL.md"),
        "---\nname: alpha\n---\n",
    );
    write(&home.path().join(".claude/commands/run.md"), "body");
    let result = detect(
        home.path(),
        None,
        &request(&["commands", "skills"]),
        &compare(),
    );
    assert_eq!(
        result["agents"][0]["categories"][0]["category"],
        json!("skills")
    );
}
