//! 原子写（rust-u2-host-skeleton.md §3.3；质量报告 §5 给对标仓库更好的落点）：
//! 同目录临时文件 → 写入并 fsync → rename（对 EPERM/EBUSY/Win32 32/33 短重试）
//! → fsync 父目录（Windows 上目录 fsync 是空操作）。rename 失败时清理临时文件。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

/// rename 的短重试次数与间隔（Windows 杀毒/索引器短暂占用目标是已知现象）。
const RENAME_ATTEMPTS: u32 = 5;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(20);

/// 把 `bytes` 原子地写入 `path`（同目录临时文件 + rename）。
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("Atomic write directory unavailable: {}", parent.display()))?;
    let temp = temp_path(path);
    let result = (|| -> Result<()> {
        let mut file = std::fs::File::create(&temp)
            .with_context(|| format!("Atomic write temp unavailable: {}", temp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("Atomic write failed: {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("Atomic write flush failed: {}", temp.display()))?;
        drop(file);
        rename_with_retry(&temp, path)?;
        sync_dir(&parent);
        Ok(())
    })();
    if result.is_err() {
        // 失败路径不留下半成品临时文件。
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "atomic".to_owned());
    let unique = format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    path.with_file_name(unique)
}

fn rename_with_retry(from: &Path, to: &Path) -> Result<()> {
    for attempt in 0..RENAME_ATTEMPTS {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let last_attempt = attempt + 1 >= RENAME_ATTEMPTS;
                if !retryable(&error) || last_attempt {
                    return Err(anyhow::Error::new(error)
                        .context(format!("Atomic rename failed: {}", to.display())));
                }
                std::thread::sleep(RENAME_RETRY_DELAY);
            }
        }
    }
    Err(anyhow::anyhow!(
        "Atomic rename exhausted retries: {}",
        to.display()
    ))
}

/// Windows：目标被短暂占用（ERROR_ACCESS_DENIED=5、ERROR_SHARING_VIOLATION=32、
/// ERROR_LOCK_VIOLATION=33）；Unix：EPERM/EACCES（目录无写权限的忙时）与 EBUSY。
fn retryable(error: &std::io::Error) -> bool {
    match error.raw_os_error() {
        Some(32 | 33) => true,
        _ => matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
        ),
    }
}

/// 父目录 fsync：保证 rename 的目录项落盘（Windows 不支持目录句柄 fsync，跳过）。
fn sync_dir(parent: &Path) {
    #[cfg(unix)]
    {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_replaces_content_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write(&path, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}");
        write(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":2}");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "无临时文件残留: {leftovers:?}");
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/state.json");
        write(&path, b"{}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }

    #[test]
    fn failed_rename_is_reported_and_temp_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        // 目标是已存在的目录：rename（文件→目录）在 Unix 与 Windows 上都是错误。
        let target = dir.path().join("occupying");
        std::fs::create_dir(&target).unwrap();
        let error = write(&target, b"{}").unwrap_err();
        assert!(
            error.to_string().contains("Atomic rename failed"),
            "{error:#}"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "失败路径不留临时文件: {leftovers:?}");
    }
}
