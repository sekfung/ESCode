//! git 域的路径规则（TS `git/config.ts`）与 Node `path` 语义（`resolve` / `relative` / `isAbsolute`）。
//!
//! Node 的 `path` 在 POSIX 与 Windows 上是两套实现；这里按编译目标选择（与 TS 在同平台上的
//! 行为一致）。Windows 分支覆盖盘符绝对路径与 `\\`/`/` 混用；UNC 路径按普通绝对路径处理
//! （差异只在 UNC 工作区上可见）。

/// JS `String.prototype.trim`（含 U+FEFF，不含 U+0085）。
pub fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_space)
}

fn is_js_space(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

/// JS `String.prototype.trimEnd` 的 `/\s+$/` 替换。
pub fn js_trim_end(value: &str) -> &str {
    value.trim_end_matches(is_js_space)
}

/// TS `normalizeGitPath`：反斜杠转正斜杠。
pub fn normalize_git_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn strip_dot_slash(path: &str) -> &str {
    // TS `.replace(/^\.?\//, "")`：至多去掉一个 `./` 或 `/`。
    path.strip_prefix("./")
        .or_else(|| path.strip_prefix('/'))
        .unwrap_or(path)
}

/// TS `normalizeWorkspaceInRepoPath`。
pub fn normalize_workspace_in_repo_path(path: &str) -> String {
    let normalized = normalize_git_path(path);
    let normalized = strip_dot_slash(&normalized).trim_end_matches('/');
    if normalized.is_empty() {
        ".".to_owned()
    } else {
        normalized.to_owned()
    }
}

/// TS `isPathInWorkspaceScope`。
pub fn is_path_in_workspace_scope(repo_relative_path: &str, workspace_in_repo_path: &str) -> bool {
    let normalized = normalize_git_path(repo_relative_path);
    let path = strip_dot_slash(&normalized);
    let workspace = normalize_workspace_in_repo_path(workspace_in_repo_path);
    if workspace == "." {
        return true;
    }
    path == workspace || path.starts_with(&format!("{workspace}/"))
}

/// TS `toWorkspaceRelativeGitPath`。
pub fn to_workspace_relative_git_path(
    repo_relative_path: &str,
    workspace_in_repo_path: &str,
) -> String {
    let normalized = normalize_git_path(repo_relative_path);
    let path = strip_dot_slash(&normalized);
    let workspace = normalize_workspace_in_repo_path(workspace_in_repo_path);
    if workspace == "." {
        return path.to_owned();
    }
    if path == workspace {
        return ".".to_owned();
    }
    path.strip_prefix(&format!("{workspace}/"))
        .unwrap_or(path)
        .to_owned()
}

// ── Node `path` ──

#[cfg(not(windows))]
const SEPARATOR: char = '/';
#[cfg(windows)]
const SEPARATOR: char = '\\';

/// Node `path.isAbsolute`。
pub fn is_absolute(path: &str) -> bool {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        path.starts_with(['/', '\\'])
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'))
    } else {
        path.starts_with('/')
    }
}

/// 把绝对路径拆成 (根, 段)（Windows 盘符大小写按 Node：保留原样）。
fn split_root(path: &str) -> (String, Vec<String>) {
    let unified = if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_owned()
    };
    let (root, rest) = if cfg!(windows) && unified.len() >= 2 && unified.as_bytes()[1] == b':' {
        (unified[..2].to_owned() + "\\", unified[2..].to_owned())
    } else {
        (SEPARATOR.to_string(), unified)
    };
    let segments = rest
        .split(SEPARATOR)
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    (root, segments)
}

/// Node `path.resolve(base, ...segments)`（base 必须是绝对路径；Host 的工作区路径恒为绝对）。
pub fn resolve(base: &str, segments: &[&str]) -> String {
    let mut current = base.to_owned();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        if is_absolute(segment) {
            current = (*segment).to_owned();
        } else {
            current = format!("{current}{SEPARATOR}{segment}");
        }
    }
    let (root, raw) = split_root(&current);
    let mut stack: Vec<String> = Vec::new();
    for segment in raw {
        match segment.as_str() {
            "." => {}
            ".." => {
                stack.pop();
            }
            _ => stack.push(segment),
        }
    }
    if stack.is_empty() {
        return root;
    }
    format!("{root}{}", stack.join(&SEPARATOR.to_string()))
}

/// Node `path.relative(from, to)`（两者都先 resolve；Windows 比较不区分大小写）。
pub fn relative(from: &str, to: &str) -> String {
    let from = resolve(from, &[]);
    let to = resolve(to, &[]);
    let (from_root, from_segments) = split_root(&from);
    let (to_root, to_segments) = split_root(&to);
    let equal = |a: &str, b: &str| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if !equal(&from_root, &to_root) {
        return to;
    }
    let common = from_segments
        .iter()
        .zip(&to_segments)
        .take_while(|(a, b)| equal(a, b))
        .count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from_segments.len() - common];
    parts.extend(to_segments[common..].iter().cloned());
    parts.join(&SEPARATOR.to_string())
}

/// TS `toAbsolutePath(repoRoot, repoRelativePath)`：`resolve(repoRoot, ...path.split("/"))`。
pub fn to_absolute_path(repo_root: &str, repo_relative_path: &str) -> String {
    let normalized = normalize_git_path(repo_relative_path);
    let parts: Vec<&str> = normalized.split('/').collect();
    resolve(repo_root, &parts)
}

/// TS `isAbsolute(path) ? path : resolve(workspacePath, path.split("/").join(sep))`。
pub fn absolute_from_workspace(workspace_path: &str, path: &str) -> String {
    if is_absolute(path) {
        return path.to_owned();
    }
    resolve(
        workspace_path,
        &[&path
            .split('/')
            .collect::<Vec<_>>()
            .join(&SEPARATOR.to_string())],
    )
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn node_resolve_and_relative_match_posix_semantics() {
        assert_eq!(resolve("/repo", &["a", "b"]), "/repo/a/b");
        assert_eq!(resolve("/repo", &["a/../b", ".", "c/"]), "/repo/b/c");
        assert_eq!(resolve("/repo", &["/abs", "x"]), "/abs/x");
        assert_eq!(resolve("/", &[".."]), "/");
        assert_eq!(resolve("/repo/", &[]), "/repo");
        assert_eq!(relative("/repo", "/repo/a/b"), "a/b");
        assert_eq!(relative("/repo/a", "/repo/b"), "../b");
        assert_eq!(relative("/repo", "/repo"), "");
        assert_eq!(relative("/repo/x/y", "/other"), "../../../other");
    }

    #[test]
    fn workspace_scope_rules_match_config_ts() {
        assert_eq!(normalize_workspace_in_repo_path("./pkg/a/"), "pkg/a");
        assert_eq!(normalize_workspace_in_repo_path(""), ".");
        assert_eq!(normalize_workspace_in_repo_path("/"), ".");
        assert!(is_path_in_workspace_scope("pkg/a/x.ts", "pkg/a/"));
        assert!(is_path_in_workspace_scope("pkg/a", "pkg/a"));
        assert!(!is_path_in_workspace_scope("pkg/ab/x.ts", "pkg/a"));
        assert!(is_path_in_workspace_scope("anything", "."));
        assert_eq!(
            to_workspace_relative_git_path("pkg/a/x.ts", "pkg/a"),
            "x.ts"
        );
        assert_eq!(to_workspace_relative_git_path("pkg/a", "pkg/a"), ".");
        assert_eq!(
            to_workspace_relative_git_path("other/y", "pkg/a"),
            "other/y"
        );
        assert_eq!(js_trim("\u{feff} x \n"), "x");
    }
}
