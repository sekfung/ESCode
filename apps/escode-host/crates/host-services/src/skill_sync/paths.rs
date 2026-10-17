//! skill 同步的安全相对路径（TS `skillSyncPath.ts`）。
//!
//! 跨 RPC 的 `directoryName` 不能只信 UI：绝对路径、盘符、反斜杠、`.`/`..`/空段一律拒绝。

use std::path::{Path, PathBuf};

use super::error::SkillSyncError;

/// TS `normalizeSkillSyncRelativePath`。
pub fn normalize_relative(path: &str, label: &str) -> Result<String, SkillSyncError> {
    let normalized = path
        .replace('\\', "/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned();
    let drive =
        path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':';
    let has_bad_segment = normalized
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..");
    // TS 先 replace 反斜杠、随后又因 `path.includes("\\")` 拒绝：任何反斜杠都是非法输入。
    if normalized.is_empty()
        || path.starts_with('/')
        || drive
        || path.contains('\\')
        || has_bad_segment
    {
        return Err(SkillSyncError::plain(format!("{label}: {path}")));
    }
    Ok(normalized)
}

/// TS `resolveSkillSyncPathWithin`：归一路径后限制在根内。
pub fn resolve_within(root: &Path, path: &str, label: &str) -> Result<PathBuf, SkillSyncError> {
    let normalized = normalize_relative(path, label)?;
    let target = root.join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR));
    if target == root || !target.starts_with(root) {
        return Err(SkillSyncError::plain(format!("{label}: {path}")));
    }
    Ok(target)
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
