//! 工作区扫描与默认过滤器（TS `fileService.ts` 的 `ensureWorkspaceFileIndex` 扫描段 + `workspaceFileMentionFilter.ts`）。

use std::path::Path;

use super::codec::IndexEntry;
use super::ignore_rules::{IGNORE_FILE_NAME, Rules};

const SKIPPED_FILE_NAMES: [&str; 2] = ["coverage.out", "lcov.info"];
const SKIPPED_FILE_EXTENSIONS: [&str; 29] = [
    ".a",
    ".aar",
    ".beam",
    ".class",
    ".dll",
    ".dylib",
    ".ear",
    ".exe",
    ".gcda",
    ".gcno",
    ".gem",
    ".hi",
    ".idb",
    ".ilk",
    ".jar",
    ".lib",
    ".node",
    ".nupkg",
    ".o",
    ".obj",
    ".pdb",
    ".profdata",
    ".profraw",
    ".pyc",
    ".pyo",
    ".rlib",
    ".so",
    ".tsbuildinfo",
    ".war",
];

/// Node `path.extname`：最后一个 `.` 之后（含点）；以点开头且无其它点、或以点结尾均无扩展名。
fn extname(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && index + 1 < name.len() => &name[index..],
        _ => "",
    }
}

fn should_skip_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || SKIPPED_FILE_NAMES.contains(&lower.as_str())
        || { SKIPPED_FILE_EXTENSIONS.contains(&extname(&lower)) }
}

fn inside_hidden_directory(relative: &str) -> bool {
    let mut segments: Vec<&str> = relative.split('/').collect();
    segments.pop();
    segments.iter().any(|segment| segment.starts_with('.'))
}

/// `defaultWorkspaceFileSearchFilter.evaluate(entry, {ignoreRulesActive: true})` → `(include, traverse)`。
fn evaluate(name: &str, relative: &str, directory: bool) -> (bool, bool) {
    if directory {
        // 目录黑名单已退役给 `.escodeignore`；隐藏目录不占候选但保留遍历。
        let hidden = name.starts_with('.') || inside_hidden_directory(relative);
        (!hidden, true)
    } else {
        (!should_skip_file(name), false)
    }
}

/// 遍历 `root`，返回未排序条目。`readdir` 失败：权限/不存在跳过，其余整体失败。
pub fn walk(root: &str, rules: &Rules) -> std::io::Result<Vec<IndexEntry>> {
    let mut entries = Vec::new();
    let mut pending: Vec<(std::path::PathBuf, String)> =
        vec![(Path::new(root).to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let children = match std::fs::read_dir(&directory) {
            Ok(children) => children,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        for child in children {
            let child = match child {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let name = child.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if relative == IGNORE_FILE_NAME {
                continue;
            }
            let file_type = child.file_type()?;
            let symlink = file_type.is_symlink();
            // 软链接跟随目标分类（目标不可读按文件），与 `resolveReaddirEntryType` 一致。
            let directory_entry = if file_type.is_dir() {
                true
            } else if symlink {
                std::fs::metadata(child.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            } else {
                false
            };
            if rules.ignores(&relative, directory_entry) {
                continue;
            }
            let (include, traverse) = evaluate(&name, &relative, directory_entry);
            if include {
                entries.push(IndexEntry {
                    directory: directory_entry,
                    relative_path: relative.clone(),
                });
            }
            if directory_entry && !symlink && traverse {
                pending.push((child.path(), relative));
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extname_matches_node_path_extname() {
        assert_eq!(extname("a.tar.gz"), ".gz");
        assert_eq!(extname(".bashrc"), "");
        assert_eq!(extname("name."), "");
        assert_eq!(extname("noext"), "");
        assert_eq!(extname(".a.so"), ".so");
    }

    #[test]
    fn default_filter_hides_secrets_binaries_and_hidden_directories() {
        assert!(should_skip_file(".ENV"));
        assert!(should_skip_file(".env.local"));
        assert!(should_skip_file("lib.SO"));
        assert!(should_skip_file("LCOV.info"));
        assert!(!should_skip_file("environment.ts"));
        assert_eq!(evaluate(".github", ".github", true), (false, true));
        assert_eq!(evaluate("src", ".github/src", true), (false, true));
        assert_eq!(evaluate("src", "src", true), (true, true));
        assert_eq!(evaluate("a.ts", "a.ts", false), (true, false));
    }
}
