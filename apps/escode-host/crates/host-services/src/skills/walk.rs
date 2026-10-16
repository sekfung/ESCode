//! 技能目录遍历（TS `skillDiscoveryWalk.ts`）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::policy;

/// libuv `scandir` 按字节序排序，Node 的 `readdir` 因此是有序的。
pub(super) fn sorted_entries(dir: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    Ok(entries)
}

/// TS `walkSkillMarkdownPaths`。
pub(super) fn walk(root: &Path, diagnostics: &mut Vec<Value>) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut visited = BTreeSet::new();
    while let Some((dir, depth)) = stack.pop() {
        let entries = match sorted_entries(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                diagnostics.push(scan_failed(&dir, &error));
                continue;
            }
        };
        let (mut has_skill, mut children, mut links) = (false, Vec::new(), Vec::new());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type().ok();
            if name == policy::SKILL_FILE_NAME
                && !kind.as_ref().is_some_and(std::fs::FileType::is_dir)
            {
                has_skill = true;
                continue;
            }
            if !policy::should_walk(&name) {
                continue;
            }
            let path = dir.join(&name);
            match kind {
                Some(kind) if kind.is_dir() => children.push(path),
                Some(kind) if kind.is_symlink() => links.push(path),
                _ => {}
            }
        }
        if has_skill {
            found.insert(dir.join(policy::SKILL_FILE_NAME));
        }
        if depth >= policy::MAX_SCAN_DEPTH {
            continue;
        }
        stack.extend(children.into_iter().map(|child| (child, depth + 1)));
        for link in links {
            match std::fs::metadata(&link) {
                Err(error) => {
                    diagnostics.push(scan_failed(&link, &error));
                    continue;
                }
                Ok(meta) if !meta.is_dir() => continue,
                Ok(_) => {}
            }
            let canonical = std::fs::canonicalize(&link).unwrap_or_else(|_| link.clone());
            if visited.insert(canonical) {
                stack.push((link, depth + 1));
            }
        }
    }
    found
}

fn scan_failed(path: &Path, error: &std::io::Error) -> Value {
    json!({"code": "skill_scan_failed", "severity": "warning", "message": error.to_string(), "path": path.to_string_lossy()})
}
