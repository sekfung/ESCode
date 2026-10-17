//! 写权限探测（`rust-u2-sync.md` §1.6）。
use super::*;

#[test]
fn probe_creates_and_removes_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("nested/skills");
    let result = check_directory_write_access(&target);
    assert_eq!(result["ok"], json!(true));
    assert_eq!(result["path"], json!(target.to_string_lossy()));
    assert!(target.is_dir());
    let leftovers: Vec<_> = std::fs::read_dir(&target)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".escode-sync-preflight-")
        })
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn probe_reports_an_error_instead_of_throwing() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "file").unwrap();
    let result = check_directory_write_access(&blocker.join("nested"));
    assert_eq!(result["ok"], json!(false));
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
}
