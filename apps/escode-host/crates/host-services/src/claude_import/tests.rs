use super::*;
use serde_json::json;

fn write_session(root: &Path, session_id: &str, cwd: &str) -> PathBuf {
    let path = root
        .join(".claude")
        .join("projects")
        .join("-repo")
        .join(format!("{session_id}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let lines = [
        json!({"type":"user","cwd":cwd,"timestamp":100,"message":{"content":"hello"}}),
        json!({"type":"assistant","timestamp":200,"message":{"content":"world"}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(&path, lines).unwrap();
    path
}

#[test]
fn candidate_and_import_paths_follow_the_ts_layout() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_session(dir.path(), "s1", "/repo");
    let candidate = candidate_of(&source).expect("candidate");
    assert_eq!(candidate.session_id, "s1");
    assert_eq!(candidate.workspace_path, "/repo");
    assert_eq!(candidate.preview_title.as_deref(), Some("hello"));
    assert_eq!(
        build_task_id("/repo", "s1").len(),
        "claude-import-".len() + 24
    );
    assert_eq!(build_task_id("/repo", "s1"), build_task_id("/repo", "s1"));

    let data = dir.path().join("data");
    let copied = copy_session_file_to_workspace(&data, "/repo", None, &source).expect("copy");
    assert!(copied.output_path.ends_with("projects/-repo/s1.jsonl"));
    assert_eq!(copied.created_output_paths.len(), 1);
    let again = copy_session_file_to_workspace(&data, "/repo", None, &source).expect("copy");
    assert!(
        again.created_output_paths.is_empty(),
        "existing copy is reused"
    );

    let snapshot = write_legacy_snapshot(
        &data,
        "/repo",
        None,
        "task-1",
        &json!({"taskId": "task-1"}),
        &[json!({"role":"user","content":"hello"})],
    )
    .expect("snapshot");
    assert!(
        snapshot
            .ends_with("sessions/".to_owned() + &workspace_hash("/repo", None) + "/task-1.json")
    );
}

#[test]
fn worktree_and_sidechain_sessions_are_not_candidates() {
    let dir = tempfile::tempdir().unwrap();
    assert!(is_worktree_workspace("/home/u/.claude/worktrees/repo"));
    assert!(!is_worktree_workspace("/home/u/repo"));
    let path = dir
        .path()
        .join(".claude")
        .join("projects")
        .join("-repo")
        .join("side.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        json!({"type":"user","isSidechain":true,"cwd":"/repo","message":{"content":"x"}})
            .to_string(),
    )
    .unwrap();
    assert!(candidate_of(&path).is_none());
}
