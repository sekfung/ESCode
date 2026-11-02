//! `subagents/readRuntimeConfig` 快照（docs/specs/rust-u2-subagents.md §7）。

use std::cmp::Ordering;
use std::path::Path;

use serde_json::{Value, json};

use super::read_runtime_config;

fn compare(left: &str, right: &str) -> Ordering {
    left.cmp(right)
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn write_state(home: &Path, state: &Value) {
    write(
        &home.join(".escode").join("v2").join("agents-state.json"),
        &serde_json::to_string_pretty(state).unwrap(),
    );
}

async fn snapshot(home: &Path, workspace: &Path) -> Value {
    read_runtime_config(home, &workspace.to_string_lossy(), &compare).await
}

#[tokio::test]
async fn returns_user_and_project_profiles_with_state_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    write(
        &home.join(".escode").join("agents").join("reviewer.md"),
        "---\nname: reviewer\ndescription: Review\nmodel: glm/glm-5\n---\nReview the change.",
    );
    write(
        &home.join(".escode").join("agents").join("disabled.md"),
        "---\nname: Disabled\ndescription: Disabled agent\n---\nNever loaded.",
    );
    write(
        &workspace
            .join(".escode")
            .join("agents")
            .join("project.markdown"),
        "---\nname: project-agent\ndescription: Project agent\npermissionMode: bypassPermissions\n---\nProject prompt.",
    );
    write_state(
        &home,
        &json!({
            "disabledAgentIds": ["user:user:disabled"],
            "builtInModelSelectionOverrides": {
                "Explore": {"providerId": "glm", "modelId": "glm-5"}
            },
            "pluginAgentModelSelectionOverrides": {
                "plugin:demo@market:helper": {"providerId": "glm", "modelId": "glm-5"}
            }
        }),
    );

    let answer = snapshot(&home, &workspace).await;
    assert_eq!(answer["kind"], json!("ready"), "{answer}");
    let profiles = answer["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 2, "{answer}");
    assert_eq!(profiles[0]["name"], json!("reviewer"));
    assert_eq!(profiles[0]["source"], json!("user"));
    assert_eq!(profiles[0]["systemPrompt"], json!("Review the change."));
    assert_eq!(
        profiles[0]["modelSelection"],
        json!({"providerId": "glm", "modelId": "glm-5"})
    );
    assert_eq!(profiles[1]["name"], json!("project-agent"));
    assert_eq!(profiles[1]["source"], json!("project"));
    // 项目级 profile 不接受仓库输入提升权限；禁用集也只作用于 user。
    assert_eq!(profiles[1].get("permissionMode"), None, "{answer}");
    assert_eq!(
        answer["builtInModelSelectionOverrides"],
        json!({"Explore": {"providerId": "glm", "modelId": "glm-5"}})
    );
    assert_eq!(
        answer["pluginAgentModelSelectionOverrides"],
        json!({"plugin:demo@market:helper": {"providerId": "glm", "modelId": "glm-5"}})
    );
}

#[tokio::test]
async fn missing_roots_and_invalid_markdown_stay_ready() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let workspace = dir.path().join("workspace");

    let empty = snapshot(&home, &workspace).await;
    assert_eq!(
        empty,
        json!({
            "kind": "ready",
            "profiles": [],
            "builtInModelSelectionOverrides": {},
            "pluginAgentModelSelectionOverrides": {},
        })
    );

    write(
        &home.join(".escode").join("agents").join("invalid.md"),
        "---\nname: invalid\n---\nMissing description.",
    );
    let answer = snapshot(&home, &workspace).await;
    assert_eq!(answer["kind"], json!("ready"), "{answer}");
    assert_eq!(answer["profiles"], json!([]));
}

#[tokio::test]
async fn read_failures_fall_back_instead_of_returning_a_partial_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let workspace = dir.path().join("workspace");
    let agents_root = home.join(".escode").join("agents");
    write(&agents_root, "not a directory");

    let answer = snapshot(&home, &workspace).await;
    assert_eq!(answer, json!({"kind": "built-in-fallback"}));
}
