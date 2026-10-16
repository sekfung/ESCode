use super::*;

#[test]
fn scope_paths_normalize_like_ts() {
    assert_eq!(scope_path(" ./src/a.rs/ "), "src/a.rs");
    assert_eq!(scope_path("/abs/p"), "abs/p");
    assert_eq!(scope_path("a\\b"), "a/b");
}

#[test]
fn relative_paths_follow_posix_semantics() {
    assert_eq!(relative("/repo", "/repo/src/a.rs"), "src/a.rs");
    assert_eq!(relative("/repo/sub", "/repo/src/a.rs"), "../src/a.rs");
    assert!(!inside("../src/a.rs") && inside("src/a.rs") && !inside(""));
}

#[test]
fn scope_covers_absolute_and_workspace_prefixed_paths() {
    let scope = build_scope(
        "/repo/pkg",
        "/repo",
        "pkg",
        &[
            "/repo/pkg/src/a.rs".to_owned(),
            "lib/b.rs".to_owned(),
            "  ".to_owned(),
        ],
    )
    .expect("scope");
    for expected in [
        "repo/pkg/src/a.rs",
        "pkg/src/a.rs",
        "src/a.rs",
        "lib/b.rs",
        "pkg/lib/b.rs",
    ] {
        assert!(scope.contains(expected), "{expected} in {scope:?}");
    }
    assert!(
        build_scope("/w", "/w", ".", &[" ".to_owned()]).is_none(),
        "空来源 → 不过滤"
    );
}
