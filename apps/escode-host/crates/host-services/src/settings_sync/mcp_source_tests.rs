//! 外部 MCP 配置读取（`rust-u2-sync.md` §3.4）。
use super::*;
use crate::settings_sync::sources::McpFormat;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn reads_wrapped_mcp_servers_and_drops_bad_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    write(
        &path,
        r#"{"mcpServers":{"a":{"command":"x"},"empty":{"command":"y"},"bad":"nope"," ":{}}}"#,
    );
    let servers = read_source_servers(&path, McpFormat::McpServersJson);
    assert_eq!(servers.len(), 2);
    assert!(servers.contains_key("a"));
    assert!(servers.contains_key("empty"));
}

#[test]
fn normalizes_open_code_command_arrays_and_modes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    write(
        &path,
        r#"{"mcp":{
            "array":{"type":"local","command":["uvx","tool","--flag"],"env":{"A":"1"}},
            "remote":{"type":"remote","command":["x"]},
            "string":{"type":"local","command":"  uvx tool  "},
            "stdio":{"type":"custom","command":["x"],"keep":true}
        }}"#,
    );
    let servers = read_source_servers(&path, McpFormat::McpJson);
    assert_eq!(
        servers["array"],
        serde_json::json!({"command":"uvx","args":["tool","--flag"],"env":{"A":"1"},"type":"stdio"})
    );
    assert_eq!(
        servers["remote"],
        serde_json::json!({"command":"x","args":[],"type":"http"})
    );
    assert_eq!(
        servers["string"],
        serde_json::json!({"type":"stdio","command":"uvx tool"})
    );
    assert_eq!(
        servers["stdio"],
        serde_json::json!({"command":"x","args":[],"keep":true}),
        "非 local/remote 的 type 被丢弃"
    );
}

#[test]
fn reads_codex_toml_and_strips_timeout_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    write(
        &path,
        "[mcp_servers.docs]\ncommand = \"uvx\"\nargs = [\"serve\"]\nstartup_timeout_sec = 5\n",
    );
    let servers = read_source_servers(&path, McpFormat::CodexToml);
    assert_eq!(servers["docs"]["command"], serde_json::json!("uvx"));
    let stripped = strip_timeout_fields(&servers["docs"]);
    assert_eq!(
        stripped,
        serde_json::json!({"command":"uvx","args":["serve"]})
    );
    assert!(servers["docs"].get("startup_timeout_sec").is_some());
}

#[test]
fn missing_or_broken_files_read_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.json");
    assert!(read_source_servers(&missing, McpFormat::McpServersJson).is_empty());
    let broken = dir.path().join("broken.json");
    write(&broken, "{oops");
    assert!(read_source_servers(&broken, McpFormat::McpServersJson).is_empty());
    let broken_toml = dir.path().join("broken.toml");
    write(&broken_toml, "[mcp_servers");
    assert!(read_source_servers(&broken_toml, McpFormat::CodexToml).is_empty());
}
