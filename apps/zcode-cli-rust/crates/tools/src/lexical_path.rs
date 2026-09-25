//! 与 Node `path.resolve` / `path.normalize` 对齐的词法路径折叠（不解析符号链接）。
//!
//! TS 的 `resolveWorkspacePath` 只做 `normalize`（绝对输入）或 `resolve(workingDirectory, input)`（相对输入），
//! 两者都折叠 `.`/`..` 并把分隔符统一成平台形式，但不查询文件系统。工具的 `file_path` 会原样进入模型可见的
//! 工具结果（`filePath`）、App 展示与错误文案，因此这里必须复刻同一套词法结果；真实路径解析另见
//! `zcode_cli_host::realpath`（仅用于状态键与检查点等需要归一实体的比较）。
use std::path::{Component, Path, PathBuf};

/// 折叠 `.`/`..` 与重复分隔符，保留平台分隔符与盘符/根前缀。
/// 与 Node `path.normalize` 的差异只有绝对路径的尾部分隔符（Node 保留、这里去掉），工具路径不会靠它区分实体。
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            // 根前缀上的 `..` 无处可退，Node 会把它留在根（如 `C:\..\a` → `C:\a`）。
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Node `path.resolve(base, input)`：绝对输入按自身归一，相对输入接到 base 后再归一。
pub(super) fn resolve(base: &Path, input: &Path) -> PathBuf {
    normalize(&if input.is_absolute() {
        input.to_owned()
    } else {
        base.join(input)
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize, resolve};
    use std::path::{Path, PathBuf};

    /// `/` 分段的期望路径，join 时换成平台分隔符（测试在 Windows 与 POSIX 上同形）。
    fn path(text: &str) -> PathBuf {
        text.split('/')
            .fold(PathBuf::new(), |out, part| out.join(part))
    }

    #[test]
    fn folds_dots_and_separators_like_node() {
        let cwd = path("ws/sub");
        for (input, expected) in [
            ("a.txt", "ws/sub/a.txt"),
            ("./a.txt", "ws/sub/a.txt"),
            ("nested/./x", "ws/sub/nested/x"),
            ("nested/../a.txt", "ws/sub/a.txt"),
            ("../a.txt", "ws/a.txt"),
            ("../../a.txt", "a.txt"),
            ("a//b", "ws/sub/a/b"),
            ("a/b/", "ws/sub/a/b"),
        ] {
            assert_eq!(normalize(&cwd.join(input)), path(expected), "input: {input}");
        }
    }

    #[test]
    fn parent_at_root_stops_at_root() {
        let root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        assert_eq!(normalize(&root.join("..").join("a")), root.join("a"));
        assert_eq!(normalize(&root.join("a").join("..").join("..")), root);
    }

    #[test]
    fn resolve_keeps_absolute_and_rebases_relative() {
        let root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        let base = root.join("ws").join("sub");
        assert_eq!(resolve(&base, &base.join("a")), base.join("a"));
        assert_eq!(resolve(&base, Path::new("../a")), root.join("ws").join("a"));
        assert_eq!(
            resolve(&base, Path::new("nested/../a")),
            root.join("ws").join("sub").join("a")
        );
    }
}
