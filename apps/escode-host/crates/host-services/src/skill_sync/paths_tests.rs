//! 安全相对路径（`rust-u2-sync.md` §1.2/§1.3）。
use super::*;

#[test]
fn normalizes_posix_and_rejects_unsafe_paths() {
    assert_eq!(
        normalize_relative("group/skill", "unsafe").unwrap(),
        "group/skill"
    );
    assert_eq!(
        normalize_relative("/group/skill/", "unsafe")
            .unwrap_err()
            .message,
        "unsafe: /group/skill/"
    );
    for bad in [
        "", "/", "..", "../x", "a/../b", "a//b", "a\\b", "C:/x", "c:", "./a", "a/./b",
    ] {
        let error = normalize_relative(bad, "unsafe").unwrap_err();
        assert_eq!(error.message, format!("unsafe: {bad}"));
    }
}

#[test]
fn resolve_stays_inside_the_root() {
    let root = std::path::Path::new("/home/user/.escode/skills");
    assert_eq!(
        resolve_within(root, "group/skill", "unsafe").unwrap(),
        root.join("group").join("skill")
    );
    assert!(resolve_within(root, "../escape", "unsafe").is_err());
    assert!(resolve_within(root, "/abs", "unsafe").is_err());
}
