//! `gen-ui` 路径工具（`rust-u2-gen-ui.md` §2）。
use super::*;

#[test]
fn normalize_collapses_dots_and_repeated_separators() {
    assert_eq!(normalize_path("/a//b/./c"), "/a/b/c");
    assert_eq!(normalize_path("/a/b/"), "/a/b");
    assert_eq!(normalize_path("/"), "/");
    assert_eq!(normalize_path(""), ".");
    assert_eq!(normalize_path("."), ".");
    assert_eq!(normalize_path("/a/././b"), "/a/b");
    // 相对路径保持相对（normalize 不补根）。
    assert_eq!(normalize_path("a/./b"), "a/b");
    // **不**化解 `..`：调用方在此之前已被路径 schema 拦下。
    assert_eq!(normalize_path("/a/../b"), "/a/../b");
}

#[test]
fn child_of_returns_none_for_equal_or_outside_paths() {
    assert_eq!(child_of("/root", "/root/a/b").as_deref(), Some("a/b"));
    // 相等（TS 的 `!child`）与越界（TS 的 `..` / 绝对）在 TS 侧同判为越界，这里合并成 None。
    assert_eq!(child_of("/root", "/root"), None);
    assert_eq!(child_of("/root", "/other/a"), None);
    // 前缀但不同目录（`/rooted` 不是 `/root` 的子）。
    assert_eq!(child_of("/root", "/rooted/a"), None);
    // 根为 `/` 时直接取余下部分。
    assert_eq!(child_of("/", "/a/b").as_deref(), Some("a/b"));
}

#[test]
fn record_file_names_must_be_64_lowercase_hex() {
    let valid = format!("{}.json", "a".repeat(64));
    assert!(is_record_file_name(&valid));
    assert!(is_record_file_name(&format!("{}.json", "0".repeat(64))));
    // 大写十六进制不算（TS 正则只收 `[a-f0-9]`）。
    assert!(!is_record_file_name(&format!("{}.json", "A".repeat(64))));
    assert!(!is_record_file_name(&format!("{}.json", "a".repeat(63))));
    assert!(!is_record_file_name(&format!("{}.json", "a".repeat(65))));
    assert!(!is_record_file_name(&format!("{}.txt", "a".repeat(64))));
    assert!(!is_record_file_name("state.lock"));
}
