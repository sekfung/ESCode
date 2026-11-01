//! `project` channel 的目录参数归一。
//!
//! 本机 owner 沿用 `node:path` 的平台语义；远端 owner 由远端 Linux/macOS 解释，必须用
//! POSIX 规则——控制端可能运行在 Windows，不能把 `/root` 判成相对路径或归一成 `\root`。

use std::path::{Component, Path};

fn rejected(code: &str) -> (String, String) {
    ("Rejected".to_owned(), code.to_owned())
}

/// `node:path.normalize` 的词法归一（折叠 `.`/重复分隔符，按词法消解 `..`，保留根前缀）。
pub(crate) fn normalize_path(path: &str) -> String {
    let mut segments: Vec<String> = Vec::new();
    let mut prefix = String::new();
    let mut rooted = false;
    for component in Path::new(path).components() {
        match component {
            Component::Prefix(value) => prefix = value.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                // 绝对路径的 `..` 到根为止；相对路径保留无法消解的前缀。
                if segments.last().is_some_and(|last| last != "..") {
                    segments.pop();
                } else if !rooted {
                    segments.push("..".to_owned());
                }
            }
            Component::Normal(value) => segments.push(value.to_string_lossy().into_owned()),
        }
    }
    let joined = segments.join(std::path::MAIN_SEPARATOR_STR);
    if rooted {
        format!("{prefix}{}{joined}", std::path::MAIN_SEPARATOR)
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

pub(crate) fn is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
}

/// 修复原因：控制端可能运行在 Windows，而远端 owner 的路径由远端 Linux/macOS 解释。
/// 用 `std::path` 判断会把 `/root/workspace` 判成相对路径、并用 `\` 归一，目录选择器
/// 因此读不到远端 home（Windows SSH E2E 停在“目录选择器初始目录未就绪”）。
pub(crate) fn is_posix_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// POSIX 词法归一：与 `path.posix.normalize` 一致（折叠重复 `/`、消解 `.`/`..`）。
pub(crate) fn posix_normalize(path: &str) -> String {
    let absolute = is_posix_absolute(path);
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..");
                }
            }
            value => segments.push(value),
        }
    }
    let joined = segments.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// 本机 owner 的目录参数：绝对性判断与归一沿用 `node:path` 的平台语义。
pub(crate) fn normalize_local_path(path: &str) -> Result<String, (String, String)> {
    if !is_absolute(path) {
        return Err(rejected("project.absolute-path-required"));
    }
    Ok(normalize_path(path))
}

/// 远端 owner 的目录参数：同一控制端可能连到 POSIX 主机，必须用 POSIX 规则。
pub(crate) fn normalize_owner_path(path: &str, posix: bool) -> Result<String, (String, String)> {
    if !posix {
        return normalize_local_path(path);
    }
    if !is_posix_absolute(path) {
        return Err(rejected("project.absolute-path-required"));
    }
    Ok(posix_normalize(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_matches_node_path_normalize() {
        // Node `path.normalize` 也是平台相关的：Windows 输出反斜杠，POSIX 输出斜杠。
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(normalize_path("/a//b/./c"), format!("{sep}a{sep}b{sep}c"));
        assert_eq!(normalize_path("/a/../b"), format!("{sep}b"));
        assert_eq!(normalize_path("a/../../b"), format!("..{sep}b"));
        assert_eq!(normalize_path("/.."), sep.to_string());
        assert_eq!(normalize_path(""), ".");
    }

    #[test]
    fn owner_path_uses_posix_rules_for_remote_owners() {
        // 远端 owner 在 Windows 控制端上也必须保持 POSIX 语义：`/root/workspace`
        // 不能变成 `\root\workspace`，也不能被判成相对路径。
        assert_eq!(
            normalize_owner_path("/root/workspace", true).unwrap(),
            "/root/workspace"
        );
        assert_eq!(
            normalize_owner_path("/root//workspace/./a/../b/", true).unwrap(),
            "/root/workspace/b"
        );
        assert_eq!(normalize_owner_path("/..", true).unwrap(), "/");
        assert_eq!(
            normalize_owner_path("root/workspace", true).unwrap_err().1,
            "project.absolute-path-required"
        );
    }
}
