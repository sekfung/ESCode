//! 导入归档（TS `skillSyncService.ts` 的 `importArchive`）。
//!
//! 先解到临时目录再逐个复制：路径穿越、半成品目录都不能污染用户级 skills 根。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::archive::extract_archive;
use super::candidates::{
    directory_path_by_name, name_key, parse_metadata, read_skill_file, user_escode_root,
};
use super::error::SkillSyncError;
use super::paths;
use crate::skills::{Compare, policy};

/// TS `importArchive`：返回 `{results}`。
pub(super) fn import_archive(
    home: &Path,
    compare: &Compare,
    archive: &[u8],
    max_archive_bytes: u64,
) -> Result<Value, SkillSyncError> {
    let root = user_escode_root(home);
    std::fs::create_dir_all(&root)?;
    let temp_root = root.join(format!(".sync-tmp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_root)?;
    let result = (|| {
        extract_archive(archive, &temp_root, Some(max_archive_bytes))?;
        let extracted = collect_extracted_skill_directories(&temp_root, compare)?;
        let mut existing = directory_path_by_name(home)?;
        let mut results = Vec::new();
        for (directory_name, source_path) in extracted {
            let target = paths::resolve_within(&root, &directory_name, "unsafe skill sync path")?;
            let Some(content) = read_skill_file(&source_path.join(policy::SKILL_FILE_NAME))? else {
                results.push(json!({
                    "name": basename(&directory_name),
                    "directoryName": directory_name,
                    "status": "failed",
                    "error": "SKILL.md is missing",
                }));
                continue;
            };
            let (name, _) = parse_metadata(&content, &directory_name);
            let key = name_key(&name);
            if target.exists() {
                results.push(json!({
                    "name": name, "directoryName": directory_name,
                    "status": "skipped", "path": target.to_string_lossy(),
                }));
                continue;
            }
            if let Some(existing_path) = existing.get(&key) {
                results.push(json!({
                    "name": name, "directoryName": directory_name,
                    "status": "skipped", "path": existing_path.to_string_lossy(),
                }));
                continue;
            }
            match copy_tree(&source_path, &target) {
                Ok(()) => {
                    results.push(json!({
                        "name": name, "directoryName": directory_name,
                        "status": "synced", "path": target.to_string_lossy(),
                    }));
                    existing.insert(key, target);
                }
                Err(error) => results.push(json!({
                    "name": name, "directoryName": directory_name,
                    "status": "failed", "path": target.to_string_lossy(),
                    "error": error.to_string(),
                })),
            }
        }
        Ok(json!({"results": results}))
    })();
    // 无论成功失败都清掉临时目录（TS finally）。
    let _ = std::fs::remove_dir_all(&temp_root);
    result
}

/// TS `collectExtractedSkillDirectories`：遍历所有目录，含 `SKILL.md` 的成为一项。
fn collect_extracted_skill_directories(
    root: &Path,
    compare: &Compare,
) -> Result<Vec<(String, PathBuf)>, SkillSyncError> {
    let mut directories = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = stack.pop() {
        if !relative.is_empty() && directory.join(policy::SKILL_FILE_NAME).exists() {
            directories.push((relative.clone(), directory.clone()));
        }
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_dir() && policy::should_walk(&name) {
                stack.push((entry.path(), join_relative(&relative, &name)));
            }
        }
    }
    directories.sort_by(|left, right| compare(&left.0, &right.0));
    Ok(directories)
}

/// Node `fs.cp(..., {recursive: true, errorOnExist: true, force: false})` 的等价实现。
fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(std::fs::read_link(source)?, target);
        }
        #[cfg(not(unix))]
        {
            std::fs::copy(source, target)?;
            return Ok(());
        }
    }
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()
}

fn basename(directory_name: &str) -> &str {
    directory_name.rsplit('/').next().unwrap_or(directory_name)
}

fn join_relative(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}
