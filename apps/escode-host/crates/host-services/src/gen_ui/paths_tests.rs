//! `gen-ui` 路径工具（`rust-u2-gen-ui.md` §2）。
use super::*;

/// 把 POSIX 写法的样例换成平台分隔符（normalize 输出 `MAIN_SEPARATOR`，与 Node `path.normalize` 同）。
///
/// 修复：期望值原先写死 `/`，Windows 上 normalize 回 `\`（windows-latest host 测试暴露）。
fn native(path: &str) -> String {
    path.replace('/', std::path::MAIN_SEPARATOR_STR)
}

#[test]
fn normalize_collapses_dots_and_repeated_separators() {
    assert_eq!(normalize_path("/a//b/./c"), native("/a/b/c"));
    assert_eq!(normalize_path("/a/b/"), native("/a/b"));
    assert_eq!(normalize_path("/"), native("/"));
    assert_eq!(normalize_path(""), ".");
    assert_eq!(normalize_path("."), ".");
    assert_eq!(normalize_path("/a/././b"), native("/a/b"));
    // 相对路径保持相对（normalize 不补根）。
    assert_eq!(normalize_path("a/./b"), native("a/b"));
    // **不**化解 `..`：调用方在此之前已被路径 schema 拦下。
    assert_eq!(normalize_path("/a/../b"), native("/a/../b"));
}

/// `child_of` 只接收 normalize 之后的路径（调用方先规范化），样例按平台分隔符给出。
#[test]
fn child_of_returns_none_for_equal_or_outside_paths() {
    let child = |root: &str, path: &str| child_of(&native(root), &native(path));
    assert_eq!(child("/root", "/root/a/b"), Some(native("a/b")));
    // 相等（TS 的 `!child`）与越界（TS 的 `..` / 绝对）在 TS 侧同判为越界，这里合并成 None。
    assert_eq!(child("/root", "/root"), None);
    assert_eq!(child("/root", "/other/a"), None);
    // 前缀但不同目录（`/rooted` 不是 `/root` 的子）。
    assert_eq!(child("/root", "/rooted/a"), None);
    // 根为 `/` 时直接取余下部分。
    assert_eq!(child("/", "/a/b"), Some(native("a/b")));
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
