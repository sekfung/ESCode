//! Bash 输出丢失诊断（docs/specs/rust-bash-model-content.md），对齐 TS `diagnoseLostBashOutput`：
//! 输出为空且非 0 退出（137 除外）时，检查输出目录所在文件系统的可用空间与 inode。
use std::path::Path;

struct Usage {
    available_mb: u64,
    /// Windows（libuv）不报告 inode，为 None。
    inodes: Option<(u64, u64)>,
}

#[cfg(unix)]
fn usage(dir: &Path) -> Option<Usage> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` 为以 NUL 结尾的有效 C 字符串，`stat` 为可写的输出缓冲区。
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let available = u128::from(stat.f_bavail as u64) * u128::from(stat.f_bsize as u64);
    Some(Usage {
        available_mb: (available / (1024 * 1024)) as u64,
        inodes: Some((stat.f_files as u64, stat.f_ffree as u64)),
    })
}

#[cfg(windows)]
fn usage(dir: &Path) -> Option<Usage> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` 为以 NUL 结尾的 UTF-16 路径；其余输出参数允许为空。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(Usage { available_mb: available / (1024 * 1024), inodes: None })
}

/// 返回 TS 的诊断文案；空间与 inode 都充足或无法查询时为 None。
pub(crate) fn lost_output_diagnostic(output_file: &Path) -> Option<String> {
    let dir = output_file.parent()?;
    let usage = usage(dir)?;
    let shown = dir.display();
    let hint = "Free up space on this filesystem.";
    if usage.available_mb < 10 {
        return Some(format!(
            "Command output was lost: the temp filesystem at {shown} is full ({}MB free). The child process's stdout/stderr writes failed with ENOSPC. {hint}",
            usage.available_mb
        ));
    }
    match usage.inodes {
        Some((files, free)) if files > 0 && free < 1000 => Some(format!(
            "Command output was lost: the temp filesystem at {shown} is out of inodes ({free} free). The child process's stdout/stderr writes failed with ENOSPC. {hint}"
        )),
        _ => None,
    }
}
