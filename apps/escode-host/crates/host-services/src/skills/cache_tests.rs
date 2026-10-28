use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::skills::discover;

fn compare() -> discover::Compare {
    Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write_file(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, content).expect("write");
}

fn skill_markdown(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n")
}

fn discover_in(workspace: &Path, home: &Path) -> discover::Discovered {
    discover::discover(workspace, true, home, &json!({}), &compare())
}

#[test]
fn same_directory_reuses_parsed_scan() {
    let root = tempfile::tempdir().expect("root");
    write_file(
        &root.path().join("alpha").join("SKILL.md"),
        &skill_markdown("alpha", "first", "BODY"),
    );

    let mut diagnostics = Vec::new();
    let first = scan_root_cached(root.path(), &compare(), &mut diagnostics);
    let second = scan_root_cached(root.path(), &compare(), &mut diagnostics);

    assert!(Arc::ptr_eq(&first, &second), "指纹未变时必须命中同一份结果");
    assert_eq!(first.skills.len(), 1);
    assert_eq!(first.skills[0].name, "alpha");
    assert_eq!(first.skills[0].body, "BODY");
    assert!(diagnostics.is_empty());
}

#[test]
fn content_size_change_invalidates_the_entry() {
    let root = tempfile::tempdir().expect("root");
    let skill = root.path().join("alpha").join("SKILL.md");
    write_file(&skill, &skill_markdown("alpha", "short", "BODY"));

    let mut diagnostics = Vec::new();
    let first = scan_root_cached(root.path(), &compare(), &mut diagnostics);
    assert_eq!(first.skills[0].description, "short");

    write_file(
        &skill,
        &skill_markdown("alpha", "a much longer description", "UPDATED"),
    );
    let second = scan_root_cached(root.path(), &compare(), &mut diagnostics);

    assert_eq!(second.skills[0].description, "a much longer description");
    assert_eq!(second.skills[0].body, "UPDATED");
}

#[test]
fn mtime_change_invalidates_the_entry() {
    let root = tempfile::tempdir().expect("root");
    let skill = root.path().join("alpha").join("SKILL.md");
    write_file(&skill, &skill_markdown("alpha", "same-size", "AAAA"));

    let mut diagnostics = Vec::new();
    let first = scan_root_cached(root.path(), &compare(), &mut diagnostics);
    assert_eq!(first.skills[0].body, "AAAA");

    // 等过毫秒边界，制造「大小相同、mtime 不同」的原地改写。
    std::thread::sleep(std::time::Duration::from_millis(10));
    write_file(&skill, &skill_markdown("alpha", "same-size", "BBBB"));
    let second = scan_root_cached(root.path(), &compare(), &mut diagnostics);

    assert_eq!(second.skills[0].body, "BBBB");
}

#[test]
fn added_and_removed_skills_change_the_result() {
    let root = tempfile::tempdir().expect("root");
    write_file(
        &root.path().join("alpha").join("SKILL.md"),
        &skill_markdown("alpha", "", "A"),
    );

    let mut diagnostics = Vec::new();
    assert_eq!(
        scan_root_cached(root.path(), &compare(), &mut diagnostics)
            .skills
            .len(),
        1
    );

    write_file(
        &root.path().join("beta").join("SKILL.md"),
        &skill_markdown("beta", "", "B"),
    );
    assert_eq!(
        scan_root_cached(root.path(), &compare(), &mut diagnostics)
            .skills
            .len(),
        2
    );

    std::fs::remove_dir_all(root.path().join("alpha")).expect("remove");
    let remaining = scan_root_cached(root.path(), &compare(), &mut diagnostics);
    assert_eq!(remaining.skills.len(), 1);
    assert_eq!(remaining.skills[0].name, "beta");
}

#[test]
fn directories_are_isolated_and_ids_stay_distinct() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let user_skill = home.path().join(".escode").join("skills").join("alpha");
    let workspace_skill = workspace
        .path()
        .join(".escode")
        .join("skills")
        .join("alpha");
    write_file(
        &user_skill.join("SKILL.md"),
        &skill_markdown("alpha", "user", "USER"),
    );
    write_file(
        &workspace_skill.join("SKILL.md"),
        &skill_markdown("alpha", "workspace", "WORKSPACE"),
    );

    let discovered = discover_in(workspace.path(), home.path());
    let alpha: Vec<_> = discovered
        .skills
        .iter()
        .filter(|skill| skill["name"] == "alpha")
        .collect();
    assert_eq!(alpha.len(), 2, "两个目录的同名技能必须各自保留");
    let ids: std::collections::BTreeSet<String> = alpha
        .iter()
        .map(|skill| skill["id"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(ids.len(), 2, "id 由路径派生，不能互相覆盖");
    let scopes: std::collections::BTreeSet<String> = alpha
        .iter()
        .map(|skill| skill["scope"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(scopes.contains("workspace"));
    assert!(scopes.contains("user"));
}

#[test]
fn covered_user_agents_skills_stay_filtered_on_cache_hits() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_file(
        &home
            .path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        &skill_markdown("alpha", "escode", "ESCODE"),
    );
    write_file(
        &home
            .path()
            .join(".agents")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        &skill_markdown("alpha", "agents", "AGENTS"),
    );

    for _ in 0..2 {
        let discovered = discover_in(workspace.path(), home.path());
        let alpha: Vec<_> = discovered
            .skills
            .iter()
            .filter(|skill| skill["name"] == "alpha")
            .collect();
        assert_eq!(alpha.len(), 1, "~/.agents 被 ~/.escode 同名技能遮蔽");
        assert_eq!(alpha[0]["scope"], json!("user"));
        assert_eq!(alpha[0]["body"], json!("ESCODE"));
    }
}

#[test]
fn metadata_changes_invalidate_the_entry() {
    let root = tempfile::tempdir().expect("root");
    let skill_dir = root.path().join("alpha");
    write_file(
        &skill_dir.join("SKILL.md"),
        &skill_markdown("alpha", "", "BODY"),
    );
    write_file(&skill_dir.join("_meta.json"), r#"{"slug":"alpha-v1"}"#);

    let mut diagnostics = Vec::new();
    let first = scan_root_cached(root.path(), &compare(), &mut diagnostics);
    assert_eq!(first.skills[0].metadata, Some(json!({"slug": "alpha-v1"})));

    write_file(
        &skill_dir.join("_meta.json"),
        r#"{"slug":"alpha-v2","version":"2"}"#,
    );
    let second = scan_root_cached(root.path(), &compare(), &mut diagnostics);
    assert_eq!(
        second.skills[0].metadata,
        Some(json!({"slug": "alpha-v2", "version": "2"}))
    );
}
