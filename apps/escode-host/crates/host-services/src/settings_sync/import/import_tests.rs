//! `importSelected`（`rust-u2-sync.md` §3.4）。
use super::*;
use serde_json::json;

fn compare() -> Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn run(home: &std::path::Path, workspace: Option<&std::path::Path>, selections: Value) -> Value {
    import_selected(
        home,
        workspace,
        &json!({"selections": selections}),
        &compare(),
    )
}

fn skill_fixture() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/skills/alpha/SKILL.md"),
        "---\nname: alpha\n---\nbody\n",
    );
    write(&home.path().join(".claude/skills/alpha/assets/a.bin"), "x");
    home
}

#[test]
fn imports_skills_by_copy_then_skips_the_second_run() {
    let home = skill_fixture();
    let source = home.path().join(".claude/skills/alpha");
    let selection = json!({
        "agent": "claudeCode", "category": "skills", "importMode": "copy",
        "skillPaths": [source.to_string_lossy()],
    });
    let result = run(home.path(), None, json!([selection]));
    assert_eq!(result["successCount"], json!(1));
    assert_eq!(result["taskResults"][0]["status"], json!("success"));
    assert_eq!(
        result["taskResults"][0]["skillResults"][0]["status"],
        json!("imported")
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join(".escode/skills/alpha/SKILL.md")).unwrap(),
        "---\nname: alpha\n---\nbody\n"
    );
    assert!(
        home.path()
            .join(".escode/skills/alpha/assets/a.bin")
            .is_file()
    );

    let again = run(
        home.path(),
        None,
        json!([{
            "agent": "claudeCode", "category": "skills", "importMode": "copy",
            "skillPaths": [source.to_string_lossy()],
        }]),
    );
    assert_eq!(again["successCount"], json!(0));
    assert_eq!(again["skippedCount"], json!(1));
    assert_eq!(
        again["taskResults"][0]["skillResults"][0]["skipReason"],
        json!("targetExists")
    );
}

#[cfg(unix)]
#[test]
fn symlink_mode_links_directories() {
    let home = skill_fixture();
    let source = home.path().join(".claude/skills/alpha");
    let result = run(
        home.path(),
        None,
        json!([{
            "agent": "claudeCode", "category": "skills",
            "skillPaths": [source.to_string_lossy()],
        }]),
    );
    assert_eq!(result["successCount"], json!(1));
    let target = home.path().join(".escode/skills/alpha");
    let metadata = std::fs::symlink_metadata(&target).unwrap();
    assert!(metadata.file_type().is_symlink());
    assert_eq!(std::fs::read_link(&target).unwrap(), source);
}

#[test]
fn imports_to_the_project_scope_when_requested() {
    let home = skill_fixture();
    let workspace = tempfile::tempdir().unwrap();
    let source = home.path().join(".claude/skills/alpha");
    let result = run(
        home.path(),
        Some(workspace.path()),
        json!([{
            "agent": "claudeCode", "category": "skills", "importMode": "copy",
            "targetScope": "project", "skillPaths": [source.to_string_lossy()],
        }]),
    );
    assert_eq!(result["successCount"], json!(1));
    assert!(
        workspace
            .path()
            .join(".escode/skills/alpha/SKILL.md")
            .is_file()
    );
    assert!(!home.path().join(".escode/skills").exists());
}

#[test]
fn imports_commands_plugins_and_mcp_and_updates_their_configs() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".claude/commands/sub/run.md"), "command");
    write(
        &home
            .path()
            .join(".claude/plugins/plug/.escode-plugin/plugin.json"),
        r#"{"name":"plug","version":"1"}"#,
    );
    write(
        &home.path().join(".claude/settings.json"),
        r#"{"mcpServers":{"docs":{"command":"uvx","timeout":5}}}"#,
    );
    write(
        &home.path().join(".escode/cli/config.json"),
        r#"{"mcp":{"other":1},"keep":true}"#,
    );

    let command = home.path().join(".claude/commands/sub/run.md");
    let plugin = home.path().join(".claude/plugins/plug");
    let mcp = format!(
        "{}#docs",
        home.path().join(".claude/settings.json").display()
    );
    let result = run(
        home.path(),
        None,
        json!([
            {"agent": "claudeCode", "category": "commands", "importMode": "copy",
             "commandPaths": [command.to_string_lossy()]},
            {"agent": "claudeCode", "category": "plugins", "importMode": "copy",
             "pluginPaths": [plugin.to_string_lossy()]},
            {"agent": "claudeCode", "category": "mcpServers",
             "mcpServerPaths": [mcp]},
        ]),
    );
    assert_eq!(result["successCount"], json!(3), "{result}");
    assert!(home.path().join(".escode/commands/sub/run.md").is_file());
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join(".escode/cli/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["keep"], json!(true));
    assert_eq!(config["mcp"]["other"], json!(1));
    assert_eq!(config["mcp"]["servers"]["docs"]["command"], json!("uvx"));
    assert!(
        config["mcp"]["servers"]["docs"].get("timeout").is_none(),
        "外部 timeout 字段不落盘"
    );
    assert_eq!(
        config["plugins"]["dirs"],
        json!([home.path().join(".escode/plugins/plug").to_string_lossy()])
    );

    // 再跑一次：命令 targetExists、插件 sameNameExists（配置 id）、MCP sameNameExists。
    let again = run(
        home.path(),
        None,
        json!([
            {"agent": "claudeCode", "category": "commands", "importMode": "copy",
             "commandPaths": [command.to_string_lossy()]},
            {"agent": "claudeCode", "category": "plugins", "importMode": "copy",
             "pluginPaths": [plugin.to_string_lossy()]},
            {"agent": "claudeCode", "category": "mcpServers",
             "mcpServerPaths": [format!("{}#docs", home.path().join(".claude/settings.json").display())]},
        ]),
    );
    assert_eq!(again["skippedCount"], json!(3), "{again}");
    let reasons: Vec<&str> = again["taskResults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| {
            let category = task["category"].as_str().unwrap();
            let key = match category {
                "commands" => "commandResults",
                "plugins" => "pluginResults",
                _ => "mcpServerResults",
            };
            task[key][0]["skipReason"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        reasons,
        vec!["targetExists", "targetExists", "sameNameExists"]
    );
}

#[test]
fn unknown_categories_are_reported_as_skipped() {
    let home = tempfile::tempdir().unwrap();
    let result = run(
        home.path(),
        None,
        json!([{"agent": "claudeCode", "category": "providers"}]),
    );
    assert_eq!(result["skippedCount"], json!(1));
    assert_eq!(result["taskResults"][0]["status"], json!("skipped"));
    assert!(result["taskResults"][0].get("skillResults").is_none());
}
