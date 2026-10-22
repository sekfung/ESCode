//! 路径语义（Node `path.resolve` / `path.relative` / `path.join` 的子集）。
//!
//! Host 不读进程 cwd：相对 `workspacePath` 直接拒绝（`rust-u2-hooks.md` §8-1）。
//! 绝对路径的折叠只做词法处理，不访问文件系统。

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// TS `resolve(path)`（绝对路径）：折叠 `.`/`..`；相对路径拒绝。
pub fn normalize_absolute(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(format!(
            "workspacePath must be absolute (Host does not read process cwd): {value}"
        ));
    }
    Ok(normalize(path))
}

/// 纯词法折叠（保留根/前缀）。
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Node 路径转成线上字符串（Node 用平台分隔符）。
pub fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn join(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        path.push(part);
    }
    normalize(&path)
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn parent(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// TS `discoverWorkspaceHookConfigPaths`：向上找 `.git` 标记；找到则根→叶，
/// 未找到只取起点目录（`workspace-hook-config.ts:335-346`）。
pub async fn project_config_directories(start: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        directories.push(current.clone());
        if has_worktree_marker(&current).await {
            directories.reverse();
            return directories;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    vec![start.to_path_buf()]
}

async fn has_worktree_marker(directory: &Path) -> bool {
    match tokio::fs::metadata(directory.join(".git")).await {
        Ok(metadata) => metadata.is_dir() || metadata.is_file(),
        Err(_) => false,
    }
}

/// 每个目录的候选文件：`escode.json`、`.escode/config.json`；按存在过滤、按规范化路径去重。
pub async fn discover_config_files(start: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for directory in project_config_directories(start).await {
        for candidate in [
            directory.join("escode.json"),
            directory.join(".escode").join("config.json"),
        ] {
            if tokio::fs::metadata(&candidate).await.is_err() {
                continue;
            }
            let key = path_text(&normalize(&candidate));
            if seen.insert(key) {
                files.push(normalize(&candidate));
            }
        }
    }
    files
}

/// TS `normalizeRelativeSourcePath`：`path.relative(workspace, source)`，空值回退 basename，
/// 分隔符统一为 `/`。
pub fn relative_source_path(workspace: &Path, source: &Path) -> String {
    let value = node_relative(workspace, source);
    if value.is_empty() {
        file_name(source)
    } else {
        value.replace('\\', "/")
    }
}

/// TS `path.relative` 的绝对路径子集。
pub fn node_relative(from: &Path, to: &Path) -> String {
    let from = normalize(from);
    let to = normalize(to);
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let mut common = 0;
    while common < from_parts.len()
        && common < to_parts.len()
        && component_eq(from_parts[common], to_parts[common])
    {
        common += 1;
    }
    // 不同盘符/前缀：Node 返回目标绝对路径。
    if common == 0 {
        return path_text(&to);
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".to_owned());
    }
    for component in &to_parts[common..] {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join(std::path::MAIN_SEPARATOR_STR)
}

fn component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    #[cfg(windows)]
    {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left.as_os_str() == right.as_os_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_fold_dot_and_parent_segments() {
        // Windows 的绝对路径需要盘符，不能复用 POSIX 路径字符串。
        let (input, expected) = if cfg!(windows) {
            (r"C:\a\.\b\..\c", PathBuf::from(r"C:\a\c"))
        } else {
            ("/a/./b/../c", PathBuf::from("/a/c"))
        };
        let path = normalize_absolute(input).unwrap();
        assert_eq!(path, expected);
        assert!(normalize_absolute("relative/path").is_err());
    }

    #[test]
    fn relative_paths_match_node_semantics() {
        assert_eq!(
            relative_source_path(
                Path::new("/work/app"),
                Path::new("/work/app/.escode/config.json")
            ),
            ".escode/config.json"
        );
        assert_eq!(
            relative_source_path(Path::new("/work/app"), Path::new("/work/other/escode.json")),
            "../other/escode.json"
        );
        assert_eq!(
            relative_source_path(Path::new("/work/app"), Path::new("/work/app")),
            "app"
        );
    }
}
