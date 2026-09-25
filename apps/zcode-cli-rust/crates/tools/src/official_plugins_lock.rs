//! 官方插件缓存的跨进程目录锁与瞬时错误重试，对齐 TS `official-plugin-seed-lock.ts` 与
//! `official-plugin-cache-fs.ts`。见 docs/specs/rust-official-plugin-seed.md。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use zcode_cli_domain::json_order::Json;

use super::official_plugins_cache::now_ms;

const LOCK_RETRY: Duration = Duration::from_millis(50);
const STALE_LOCK_AGE: Duration = Duration::from_secs(60);
const RETRY_DELAYS_MS: [u64; 3] = [25, 50, 100];
const RETRY_BUDGET: Duration = Duration::from_millis(1500);

/// TS `withOfficialPluginSeedLock`：mkdir 互斥；陈旧锁（owner 进程不在或无 owner 且超过 60s）改名后接管。
pub(crate) fn with_lock<T>(
    target: &Path,
    deadline: Instant,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock = PathBuf::from(format!("{}.seed-lock", target.display()));
    if let Some(parent) = lock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    loop {
        match std::fs::create_dir(&lock) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                take_over_stale(&lock)?;
                if Instant::now() >= deadline {
                    bail!(
                        "[official-plugin-seed-lock] timed out waiting for {}",
                        lock.display()
                    );
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(error) => return Err(error.into()),
        }
    }
    let owner = format!(
        "{{\"createdAt\":{},\"pid\":{}}}",
        Json::str(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .compact(),
        std::process::id()
    );
    let result = std::fs::write(lock.join("owner.json"), owner)
        .map_err(anyhow::Error::from)
        .and_then(|()| action());
    let _ = std::fs::remove_dir_all(&lock);
    result
}

fn take_over_stale(lock: &Path) -> Result<()> {
    let age = match std::fs::metadata(lock).and_then(|m| m.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let owner = std::fs::read_to_string(lock.join("owner.json"))
        .ok()
        .and_then(|t| Json::parse(&t))
        .and_then(|o| match o.get("pid") {
            Some(Json::Number(n)) => n.as_u64().filter(|p| *p > 0),
            _ => None,
        });
    if owner.is_some_and(|pid| process_alive(pid as u32))
        || (owner.is_none() && age < STALE_LOCK_AGE)
    {
        return Ok(());
    }
    let stale = PathBuf::from(format!(
        "{}.stale-{}-{}",
        lock.display(),
        std::process::id(),
        now_ms()
    ));
    match std::fs::rename(lock, &stale) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&stale);
            Ok(())
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::AlreadyExists
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::DirectoryNotEmpty
                    | std::io::ErrorKind::ResourceBusy
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) 只探测进程是否存在。EPERM 表示存在但不可探测。
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: 只打开句柄探测存在性并立即关闭；拒绝访问同样说明进程存在。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return std::io::Error::last_os_error().raw_os_error() == Some(5);
        }
        CloseHandle(handle);
        true
    }
}

/// TS retryOfficialPluginCacheFs：瞬时错误按 25/50/100ms 重试，总预算 1.5s。
pub(crate) fn retry(mut operation: impl FnMut() -> std::io::Result<()>) -> Result<()> {
    let deadline = Instant::now() + RETRY_BUDGET;
    let mut attempt = 0;
    loop {
        match operation() {
            Ok(()) => return Ok(()),
            Err(error) => {
                let delay = RETRY_DELAYS_MS
                    .get(attempt)
                    .map(|ms| Duration::from_millis(*ms));
                attempt += 1;
                match delay {
                    Some(delay) if transient_io(&error) && Instant::now() + delay <= deadline => {
                        std::thread::sleep(delay);
                    }
                    _ => return Err(error.into()),
                }
            }
        }
    }
}

pub(crate) fn remove_dir(path: &Path) -> Result<()> {
    retry(|| match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

fn transient_io(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::AlreadyExists
            | std::io::ErrorKind::DirectoryNotEmpty
            | std::io::ErrorKind::ResourceBusy
    )
}

pub(crate) fn is_transient(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(transient_io)
}

pub(crate) fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
