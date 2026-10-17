//! Claude AGENTS.md 迁移（`rust-u2-sync.md` §3）。
use super::*;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn missing_source_is_reported_and_copy_is_skipped() {
    let home = tempfile::tempdir().unwrap();
    let status = migration_status(home.path()).await.unwrap();
    assert!(!status.to_json()["supported"].as_bool().unwrap());
    assert_eq!(
        status.to_json()["unavailableReason"],
        serde_json::json!("missingSource")
    );

    let result = copy(home.path(), false).await.unwrap();
    let value = result.to_json();
    assert_eq!(value["status"], serde_json::json!("skipped"));
    assert_eq!(value["skippedReason"], serde_json::json!("missingSource"));
    assert!(!home.path().join(".escode/AGENTS.md").exists());
}

#[tokio::test]
async fn copies_and_respects_overwrite() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join(".claude/CLAUDE.md");
    write(&source, "claude instructions");
    let target = home.path().join(".escode/AGENTS.md");

    let status = migration_status(home.path()).await.unwrap().to_json();
    assert_eq!(status["sourceExists"], serde_json::json!(true));
    assert_eq!(status["targetExists"], serde_json::json!(false));
    assert_eq!(status["supported"], serde_json::json!(true));

    let copied = copy(home.path(), false).await.unwrap().to_json();
    assert_eq!(copied["status"], serde_json::json!("copied"));
    assert_eq!(copied["overwritten"], serde_json::json!(false));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "claude instructions"
    );

    write(&target, "existing");
    let skipped = copy(home.path(), false).await.unwrap().to_json();
    assert_eq!(skipped["status"], serde_json::json!("skipped"));
    assert_eq!(skipped["skippedReason"], serde_json::json!("targetExists"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "existing");

    let overwritten = copy(home.path(), true).await.unwrap().to_json();
    assert_eq!(overwritten["status"], serde_json::json!("copied"));
    assert_eq!(overwritten["overwritten"], serde_json::json!(true));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "claude instructions"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_broken_target_symlink_counts_as_existing() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".claude/CLAUDE.md"), "x");
    std::fs::create_dir_all(home.path().join(".escode")).unwrap();
    std::os::unix::fs::symlink("/nonexistent", home.path().join(".escode/AGENTS.md")).unwrap();
    let result = copy(home.path(), false).await.unwrap().to_json();
    assert_eq!(result["status"], serde_json::json!("skipped"));
    assert_eq!(result["skippedReason"], serde_json::json!("targetExists"));
}
