//! MCP 目录路径（`rust-u2-sync.md` §2）。
use super::*;

#[test]
fn user_and_workspace_paths_follow_the_descriptors() {
    let home = std::path::Path::new("/home/me");
    assert_eq!(
        config_path(ESCODE, Scope::User, home, None).unwrap(),
        home.join(".escode/cli/config.json")
    );
    assert_eq!(
        config_path(AGENTS, Scope::User, home, None).unwrap(),
        home.join(".agents/mcp.json")
    );
    assert_eq!(
        config_path(ESCODE, Scope::Workspace, home, Some("/work")).unwrap(),
        std::path::Path::new("/work/.escode/config.json")
    );
    assert_eq!(
        config_path(AGENTS, Scope::Workspace, home, None).unwrap_err(),
        "Missing workspace path for agents workspace MCP config"
    );
}

#[test]
fn locations_carry_the_project_path_only_when_provided() {
    let home = std::path::Path::new("/home/me");
    let user = location(ESCODE, Scope::User, home, Some("/work")).unwrap();
    assert_eq!(user["scope"], serde_json::json!("user"));
    assert_eq!(user["projectPath"], serde_json::json!("/work"));
    let user = location(ESCODE, Scope::User, home, None).unwrap();
    assert!(user.get("projectPath").is_none());
    let workspace = location(AGENTS, Scope::Workspace, home, Some("/work")).unwrap();
    assert_eq!(workspace["scope"], serde_json::json!("project"));
    assert_eq!(
        workspace["directoryPath"],
        serde_json::json!("/work/.agents")
    );
}

#[test]
fn unknown_sources_are_rejected_and_locations_parse() {
    assert_eq!(
        descriptor_for_source("claude").unwrap_err(),
        "Unsupported MCP settings directory source: claude"
    );
    let parsed = Location::from_json(&serde_json::json!({
        "source": "agents", "scope": "project",
        "directoryPath": "/work/.agents", "projectPath": "/work",
    }));
    assert_eq!(parsed.source, "agents");
    assert_eq!(parsed.scope, Scope::Workspace);
    assert_eq!(parsed.project_path.as_deref(), Some("/work"));
}
