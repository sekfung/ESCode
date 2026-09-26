//! 与 Node 共用的跨进程文件锁与私有文件原子写（docs/specs/rust-mcp-oauth.md，对齐 TS
//! `@zcode/shared/node` 的 `atomicFileLock.ts` / `privateFilePersistence.ts`）。
//!
//! 锁是 `<file>.lock` 目录，内含唯一 `owner-<token>.json`（`{pid, createdAt, token}`）。owner 进程已退出，
//! 或无 PID 且超过 grace，才可回收；等待上限 8s，重试间隔 25/50/100/200/400ms。
use anyhow::{Context, Result, bail};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RETRY_DELAYS_MS: [u64; 5] = [25, 50, 100, 200, 400];
const OWNERLESS_GRACE_MS: u64 = 100;
const MAX_WAIT_MS: u64 = 8_000;
const MAX_CLOCK_SKEW_MS: u64 = 5 * 60_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Node `process.kill(pid, 0)`：只有「进程不存在」才算退出，权限不足等其它错误视为仍存活。
fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        // SAFETY: 只查询并关闭本函数打开的句柄。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            // libuv uv_kill(pid, 0)：已退出（退出码不是 STILL_ACTIVE）按 ESRCH 处理。
            ok == 0 || code == STILL_ACTIVE as u32
        }
    }
    #[cfg(not(windows))]
    {
        // SAFETY: 信号 0 只做存在性检查。
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}

fn valid_timestamp(value: Option<u64>, observed: u64) -> Option<u64> {
    value.filter(|v| *v <= observed + MAX_CLOCK_SKEW_MS)
}
fn mtime_ms(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// TS `createLockInstanceObserver` 的近似：时间戳不可用时以首次观察到该路径的时刻代替。
static FIRST_SEEN: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Default::default);
fn first_seen(path: &Path, observed: u64) -> u64 {
    *FIRST_SEEN
        .lock()
        .unwrap()
        .entry(path.to_owned())
        .or_insert(observed)
}

async fn owner_reclaimable(owner: &Path) -> Result<bool> {
    let observed = now_ms();
    let raw = tokio::fs::read_to_string(owner).await?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    let pid = parsed["pid"]
        .as_f64()
        .filter(|p| *p > 0.0 && p.fract() == 0.0 && *p <= 9_007_199_254_740_991.0)
        .map(|p| p as u64);
    let created = match valid_timestamp(
        parsed["createdAt"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| v as u64),
        observed,
    ) {
        Some(created) => created,
        None => {
            let meta = tokio::fs::metadata(owner).await?;
            valid_timestamp(mtime_ms(&meta), observed)
                .unwrap_or_else(|| first_seen(owner, observed))
        }
    };
    let exited = pid.is_some_and(|pid| u32::try_from(pid).map_or(true, |pid| !process_alive(pid)));
    let stale = pid.is_none() && observed.saturating_sub(created) >= OWNERLESS_GRACE_MS;
    Ok(exited || stale)
}

fn is_owner(name: &str) -> bool {
    name.starts_with("owner-") && name.ends_with(".json")
}

/// TS `removeAbandonedLock`：只回收可证明已放弃的锁；返回是否删除。
async fn remove_abandoned(lock: &Path) -> Result<bool> {
    let meta = match tokio::fs::metadata(lock).await {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_dir() {
        // 兼容升级前的单文件锁。
        let raw = tokio::fs::read_to_string(lock).await?;
        if !owner_reclaimable(lock).await? || tokio::fs::read_to_string(lock).await? != raw {
            return Ok(false);
        }
        tokio::fs::remove_file(lock).await?;
        return Ok(true);
    }
    let mut entries = vec![];
    let mut dir = tokio::fs::read_dir(lock).await?;
    while let Some(entry) = dir.next_entry().await? {
        entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    let owners: Vec<&String> = entries.iter().filter(|e| is_owner(e)).collect();
    if owners.len() == 1 {
        let owner = lock.join(owners[0]);
        if !owner_reclaimable(&owner).await? {
            return Ok(false);
        }
        let _ = tokio::fs::remove_file(&owner).await;
        return Ok(tokio::fs::remove_dir(lock).await.is_ok());
    }
    let observed = now_ms();
    let stamp =
        valid_timestamp(mtime_ms(&meta), observed).unwrap_or_else(|| first_seen(lock, observed));
    if observed.saturating_sub(stamp) < OWNERLESS_GRACE_MS {
        return Ok(false);
    }
    for owner in &owners {
        if !owner_reclaimable(&lock.join(owner)).await? {
            return Ok(false);
        }
    }
    for entry in &entries {
        let _ = tokio::fs::remove_file(lock.join(entry)).await;
    }
    Ok(tokio::fs::remove_dir(lock).await.is_ok())
}

pub struct FileLockGuard {
    lock: PathBuf,
    owner: PathBuf,
    _local: tokio::sync::OwnedMutexGuard<()>,
}
impl FileLockGuard {
    pub async fn release(self) {
        let _ = tokio::fs::remove_file(&self.owner).await;
        let _ = tokio::fs::remove_dir(&self.lock).await;
    }
}

/// 进程内 FIFO（TS processFileLockTails）：每个进程只让队首竞争 OS 锁。
static LOCAL: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

/// TS `withFileLock` 的获取部分；调用方完成后必须 `release`。
pub async fn acquire(file: &Path) -> Result<FileLockGuard> {
    let local = LOCAL
        .lock()
        .unwrap()
        .entry(file.to_owned())
        .or_default()
        .clone();
    let local = local.lock_owned().await;
    if let Some(parent) = file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let lock = PathBuf::from(format!("{}.lock", file.display()));
    let token = format!("{}-{}-{}", std::process::id(), now_ms(), crate::id());
    let owner = lock.join(format!("owner-{token}.json"));
    let payload = format!(
        "{}\n",
        serde_json::json!({"pid":std::process::id(),"createdAt":now_ms(),"token":token})
    );
    let started = now_ms();
    for attempt in 0.. {
        match tokio::fs::create_dir(&lock).await {
            Ok(()) => {
                let written = async {
                    use tokio::io::AsyncWriteExt;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&owner)
                        .await?;
                    file.write_all(payload.as_bytes()).await?;
                    let mut owners = vec![];
                    let mut dir = tokio::fs::read_dir(&lock).await?;
                    while let Some(entry) = dir.next_entry().await? {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if is_owner(&name) {
                            owners.push(name);
                        }
                    }
                    anyhow::ensure!(
                        owners == [format!("owner-{token}.json")],
                        "ZCode file lock ownership changed during acquire"
                    );
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                if written.is_ok() {
                    return Ok(FileLockGuard {
                        lock,
                        owner,
                        _local: local,
                    });
                }
                let _ = tokio::fs::remove_file(&owner).await;
                let _ = tokio::fs::remove_dir(&lock).await;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).context("Unable to create ZCode file lock"),
        }
        let elapsed = now_ms().saturating_sub(started);
        if elapsed >= MAX_WAIT_MS {
            bail!(
                "Timed out after {elapsed}ms waiting for the ZCode file lock: {}",
                lock.display()
            );
        }
        if remove_abandoned(&lock).await.unwrap_or(false) {
            continue;
        }
        let delay = RETRY_DELAYS_MS[attempt.min(RETRY_DELAYS_MS.len() - 1)];
        tokio::time::sleep(Duration::from_millis(delay.min(MAX_WAIT_MS - elapsed))).await;
    }
    unreachable!()
}

/// TS `atomicWritePrivateTextFile`：同目录临时文件（0600）+ rename，Windows 占用时有限重试。
pub async fn write_private(file: &Path, content: &str) -> Result<()> {
    let dir = file.parent().context("Private file has no directory")?;
    tokio::fs::create_dir_all(dir).await?;
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(
        ".{name}.{}.{}.{}.tmp",
        std::process::id(),
        now_ms(),
        crate::id()
    ));
    let written = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        use tokio::io::AsyncWriteExt;
        let mut handle = options.open(&temp).await?;
        handle.write_all(content.as_bytes()).await?;
        handle.sync_all().await?;
        drop(handle);
        for delay in [50, 100, 200, 400, 800, 0] {
            match tokio::fs::rename(&temp, file).await {
                Ok(()) => return Ok(()),
                Err(e) if delay > 0 && matches!(e.kind(), std::io::ErrorKind::PermissionDenied) => {
                    tokio::time::sleep(Duration::from_millis(delay)).await
                }
                Err(e) => return Err(anyhow::Error::from(e)),
            }
        }
        Ok(())
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}
