//! MCP 目录读取与保存（`rust-u2-sync.md` §2）。
use super::*;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn load_prefers_escode_and_lists_workspace_before_user() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let workspace = dir.path().join("work");
    write(
        &workspace.join(".escode/config.json"),
        r#"{"mcp":{"servers":{"ws":{"command":"ws-command"}}}}"#,
    );
    write(
        &home.join(".agents/mcp.json"),
        r#"{"mcpServers":{"user-agents":{"command":"u"}}}"#,
    );

    let loaded = load(
        home,
        &serde_json::json!({"workspacePath": workspace.to_string_lossy()}),
    )
    .unwrap();
    let servers = loaded["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["name"], serde_json::json!("ws"));
    assert_eq!(servers[0]["scope"], serde_json::json!("workspace"));
    assert_eq!(
        servers[0]["projectPath"],
        serde_json::json!(workspace.to_string_lossy())
    );
    assert_eq!(servers[0]["file"]["format"], serde_json::json!("json"));
    assert_eq!(
        servers[0]["file"]["filePath"],
        serde_json::json!(workspace.join(".escode/config.json").to_string_lossy())
    );
    assert_eq!(
        servers[0]["location"]["scope"],
        serde_json::json!("project")
    );
    assert_eq!(servers[1]["name"], serde_json::json!("user-agents"));
    assert_eq!(servers[1]["scope"], serde_json::json!("user"));
    assert_eq!(servers[1]["source"], serde_json::json!("escodeagentmcp"));
    assert_eq!(
        servers[1]["location"]["projectPath"],
        serde_json::json!(workspace.to_string_lossy()),
        "user 记录只要传了 workspacePath 也带 projectPath"
    );

    // 用户级 `.escode` 有 server 时不再回退 `.agents`。
    write(
        &home.join(".escode/cli/config.json"),
        r#"{"mcp":{"servers":{"user-escode":{"command":"e"}}}}"#,
    );
    let loaded = load(home, &serde_json::json!({})).unwrap();
    let names: Vec<_> = loaded["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|server| server["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, vec!["user-escode"]);
}

#[test]
fn save_upserts_and_deletes_while_preserving_other_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let file = home.join(".escode/cli/config.json");
    write(&file, r#"{"mcp":{"other":1},"keep":true}"#);

    save(
        home,
        &serde_json::json!({
            "action": "upsert", "name": "srv",
            "config": {"command": "npx", "args": ["x"]},
        }),
    )
    .unwrap();
    let parsed = read(&file);
    assert_eq!(
        parsed["mcp"]["servers"]["srv"]["command"],
        serde_json::json!("npx")
    );
    assert_eq!(parsed["mcp"]["other"], serde_json::json!(1));
    assert_eq!(parsed["keep"], serde_json::json!(true));
    assert!(std::fs::read_to_string(&file).unwrap().ends_with('\n'));

    assert_eq!(
        save(
            home,
            &serde_json::json!({"action": "upsert", "name": "missing-config"})
        )
        .unwrap_err(),
        "Missing MCP config for upsert action"
    );

    save(
        home,
        &serde_json::json!({"action": "delete", "name": "srv"}),
    )
    .unwrap();
    let parsed = read(&file);
    assert!(parsed["mcp"]["servers"].get("srv").is_none());
    assert_eq!(parsed["mcp"]["other"], serde_json::json!(1));
}

#[test]
fn set_enabled_rewrites_the_server_and_cleans_the_legacy_override() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let file = home.join(".escode/cli/config.json");
    let directory = home.join(".escode/cli").to_string_lossy().into_owned();
    write(
        &file,
        &serde_json::json!({
            "mcp": {
                "servers": {"srv": {"command": "x", "enable": false}},
                (directory): {"srv": true},
            },
        })
        .to_string(),
    );

    save(
        home,
        &serde_json::json!({
            "action": "set-enabled", "name": "srv", "enabled": true,
        }),
    )
    .unwrap();
    let parsed = read(&file);
    assert_eq!(
        parsed["mcp"]["servers"]["srv"],
        serde_json::json!({"command": "x"}),
        "启用态不落冗余字段，并清掉 legacy enable"
    );
    assert_eq!(
        parsed["mcp"],
        serde_json::json!({"servers": {"srv": {"command": "x"}}}),
        "legacy 目录覆盖被清掉"
    );

    save(
        home,
        &serde_json::json!({
            "action": "set-enabled", "name": "srv", "enabled": false,
        }),
    )
    .unwrap();
    assert_eq!(
        read(&file)["mcp"]["servers"]["srv"],
        serde_json::json!({"command": "x", "enabled": false})
    );

    assert_eq!(
        save(
            home,
            &serde_json::json!({"action": "set-enabled", "name": "srv"})
        )
        .unwrap_err(),
        "Missing enabled value for MCP set-enabled action"
    );

    // 不存在的 server 不写文件。
    let before = std::fs::read_to_string(&file).unwrap();
    save(
        home,
        &serde_json::json!({
            "action": "set-enabled", "name": "nope", "enabled": false,
        }),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}
