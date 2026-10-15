use super::*;

#[test]
fn labels_strip_the_sixteen_hex_suffix() {
    assert_eq!(workspace_label("my-project-0123456789abcdef"), "my-project");
    assert_eq!(
        workspace_label("my-project-0123456789abcde"),
        "my-project-0123456789abcde"
    );
    assert_eq!(
        workspace_label("-0123456789abcdef"),
        "-0123456789abcdef",
        "空 slug 回退到 id"
    );
    assert_eq!(workspace_label("plain"), "plain");
}

#[test]
fn segment_and_file_name_rules() {
    assert!(is_valid_segment("a-1"));
    for bad in ["", ".", "..", "a/b", "a\\b"] {
        assert!(!is_valid_segment(bad), "{bad}");
    }
    assert!(is_memory_file("MEMORY.md") && is_memory_file("note.md"));
    assert!(!is_memory_file("note.txt") && !is_memory_file("MEMORY.MD"));
}

#[test]
fn list_is_empty_when_the_root_is_missing_and_rejects_a_symlinked_root() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("projects");
    assert_eq!(list(&root).expect("missing"), json!([]));
    #[cfg(unix)]
    {
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("mkdir");
        std::os::unix::fs::symlink(&real, &root).expect("symlink");
        let error = list(&root).expect_err("symlinked root");
        assert!(
            error.message.contains("not a regular directory"),
            "{}",
            error.message
        );
    }
}
