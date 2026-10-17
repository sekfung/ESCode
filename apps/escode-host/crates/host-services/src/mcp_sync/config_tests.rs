//! MCP 配置 JSON 读写与 `enable` 迁移（`rust-u2-sync.md` §2）。
use super::*;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn read_json_object_maps_errors_like_the_typescript_service() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    assert!(read_json_object(&missing).unwrap().is_none());

    let broken = dir.path().join("broken.json");
    write(&broken, "{oops");
    assert!(
        read_json_object(&broken)
            .unwrap_err()
            .starts_with("无法解析 MCP 配置文件")
    );

    let array = dir.path().join("array.json");
    write(&array, "[]");
    assert_eq!(
        read_json_object(&array).unwrap_err(),
        format!("MCP 配置文件 {} 必须是 JSON 对象", array.display())
    );
}

#[test]
fn server_maps_round_trip_without_dropping_other_fields() {
    let parsed = serde_json::json!({
        "mcp": {"other": 1, "servers": {"a": {"command": "x"}}},
        "top": 2,
    })
    .as_object()
    .unwrap()
    .clone();
    let servers = read_server_map(&parsed, ConfigKey::McpServers);
    assert_eq!(servers.len(), 1);
    let next = write_server_map(&parsed, ConfigKey::McpServers, servers);
    assert_eq!(next["mcp"]["other"], serde_json::json!(1));
    assert_eq!(
        next["mcp"]["servers"]["a"]["command"],
        serde_json::json!("x")
    );
    assert_eq!(next["top"], serde_json::json!(2));

    let agents = serde_json::json!({"mcpServers": {"b": {}}, "keep": true})
        .as_object()
        .unwrap()
        .clone();
    let servers = read_server_map(&agents, ConfigKey::Plain);
    let next = write_server_map(&agents, ConfigKey::Plain, servers);
    assert_eq!(next["keep"], serde_json::json!(true));
    assert!(next["mcpServers"]["b"].is_object());
}

#[test]
fn enabled_flags_and_legacy_migration_follow_the_conflict_rule() {
    assert!(enabled(&serde_json::json!({})));
    assert!(!enabled(&serde_json::json!({"enabled": false})));
    assert!(
        enabled(&serde_json::json!({"enabled": 0})),
        "只认布尔 false"
    );
    assert_eq!(
        set_enabled(&serde_json::json!({"enable": false, "command": "x"}), true),
        serde_json::json!({"command": "x"})
    );
    assert_eq!(
        set_enabled(&serde_json::json!({"enabled": true, "command": "x"}), false),
        serde_json::json!({"command": "x", "enabled": false})
    );

    let servers = serde_json::json!({
        "conflict": {"enable": false, "enabled": true},
        "keep": {"command": "x"},
    })
    .as_object()
    .unwrap()
    .clone();
    let (migrated, changed) = migrate_legacy(servers);
    assert!(changed);
    assert_eq!(migrated["conflict"], serde_json::json!({"enabled": false}));
    assert_eq!(migrated["keep"], serde_json::json!({"command": "x"}));

    let servers = serde_json::json!({"keep": {"command": "x"}})
        .as_object()
        .unwrap()
        .clone();
    let (_, changed) = migrate_legacy(servers);
    assert!(!changed);
}

#[test]
fn write_json_is_atomic_private_and_newline_terminated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cli/config.json");
    let object = serde_json::json!({"mcp": {"servers": {"a": {}}}})
        .as_object()
        .unwrap()
        .clone();
    write_json(&path, &object).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.starts_with("{\n"));
    assert!(raw.ends_with("\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "配置含 secret，必须是 0600");
    }
}

#[test]
fn legacy_override_removal_keeps_the_mcp_object() {
    let mut object = serde_json::json!({
        "mcp": {"/home/me/.escode/cli": {"a": true, "b": false}},
        "keep": 1,
    })
    .as_object()
    .unwrap()
    .clone();
    assert!(remove_legacy_override(
        &mut object,
        "/home/me/.escode/cli",
        "a"
    ));
    assert_eq!(
        object["mcp"]["/home/me/.escode/cli"],
        serde_json::json!({"b": false})
    );
    assert!(remove_legacy_override(
        &mut object,
        "/home/me/.escode/cli",
        "b"
    ));
    assert_eq!(object["mcp"], serde_json::json!({}));
    assert!(!remove_legacy_override(&mut object, "/nope", "a"));
}
