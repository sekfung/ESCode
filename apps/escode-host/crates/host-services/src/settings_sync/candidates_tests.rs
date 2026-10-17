//! 候选收集（`rust-u2-sync.md` §3.4）。
use super::*;

fn compare() -> Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/skills/alpha/SKILL.md"),
        "---\nname: alpha\n---\n",
    );
    write(
        &home.path().join(".claude/commands/run.md"),
        "---\ndescription: d\n---\n",
    );
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
        &workspace.path().join(".trae/skills/beta/SKILL.md"),
        "---\nname: beta\n---\n",
    );
    write(
        &workspace.path().join(".trae/settings.json"),
        r#"{"mcpServers":{"ws":{"command":"x"}}}"#,
    );
    (home, workspace)
}

#[test]
fn collects_skills_with_scope_and_target_paths() {
    let (home, workspace) = fixture();
    let candidates = collect_skill_candidates(home.path(), Some(workspace.path()), &compare());
    assert_eq!(candidates.len(), 2);
    let alpha = &candidates[0];
    assert_eq!(alpha.agent, Agent::ClaudeCode);
    assert_eq!(alpha.source_root_scope, SourceScope::User);
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.target_path, home.path().join(".escode/skills/alpha"));
    let beta = &candidates[1];
    assert_eq!(
        beta.agent,
        Agent::Trae,
        "traeCn 的项目根与 trae 相同，只取第一次"
    );
    assert_eq!(beta.source_root_scope, SourceScope::Workspace);
    assert_eq!(
        beta.target_path,
        workspace.path().join(".escode/skills/beta")
    );
}

#[test]
fn collects_commands_plugins_and_mcp() {
    let (home, workspace) = fixture();
    let commands = collect_command_candidates(home.path(), Some(workspace.path()), &compare());
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "/run");
    assert_eq!(commands[0].source_relative_path, "run.md");
    assert_eq!(
        commands[0].target_path,
        home.path().join(".escode/commands/run.md")
    );

    let plugins = collect_plugin_candidates(home.path(), Some(workspace.path()), &compare());
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id, "plug@inline");
    assert_eq!(plugins[0].version.as_deref(), Some("1"));
    assert_eq!(
        plugins[0].target_path,
        home.path().join(".escode/plugins/plug")
    );

    let mcp = collect_mcp_candidates(home.path(), Some(workspace.path()));
    assert_eq!(mcp.len(), 2);
    let docs = mcp
        .iter()
        .find(|candidate| candidate.name == "docs")
        .unwrap();
    assert_eq!(docs.source_root_scope, SourceScope::User);
    assert!(
        docs.config.get("timeout").is_none(),
        "timeout 在候选阶段剥离"
    );
    assert!(docs.source_path.ends_with("settings.json#docs"));
    let ws = mcp.iter().find(|candidate| candidate.name == "ws").unwrap();
    assert_eq!(ws.source_root_scope, SourceScope::Workspace);
}

#[test]
fn without_a_workspace_only_user_roots_are_scanned() {
    let (home, _workspace) = fixture();
    let skills = collect_skill_candidates(home.path(), None, &compare());
    assert_eq!(skills.len(), 1);
    assert!(
        skills
            .iter()
            .all(|candidate| candidate.agent == Agent::ClaudeCode)
    );
    let mcp = collect_mcp_candidates(home.path(), None);
    assert_eq!(mcp.len(), 1);
}
