//! 文件 IO 基础：原子写（临时文件 + rename）与 scope 写锁（spec §7）。
//! 锁用 `create_dir` 的原子性实现，跨进程有效（Desktop runtime 与 `escode-cli-rust wiki` 命令可能并发）。
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 陈旧锁阈值：持锁进程崩溃后，超过该时间的锁可被回收。
pub const STALE_LOCK: Duration = Duration::from_secs(120);
const LOCK_RETRIES: u32 = 50;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(100);

/// 原子写：同目录临时文件写完并 fsync 后 rename 覆盖；读者永远看不到半截文件。
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    tokio::fs::create_dir_all(dir).await?;
    let name = path
        .file_name()
        .context("path has no file name")?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), unique()));
    {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&tmp).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
    }
    if let Err(error) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(error.into());
    }
    Ok(())
}

fn unique() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    nanos ^ u128::from(COUNTER.fetch_add(1, Ordering::Relaxed))
}

pub async fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub async fn append_line(path: &Path, line: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(format!("{line}\n").as_bytes()).await?;
    Ok(())
}

/// 列目录下指定扩展名的文件（不递归，不跟随目录符号链接）。
pub async fn list_files(dir: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if entry.file_type().await?.is_file() && path.extension().is_some_and(|e| e == extension) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// scope 写锁；Drop 时释放。
pub struct ScopeLock {
    dir: PathBuf,
}

#[derive(Debug)]
pub struct WikiLocked;

impl std::fmt::Display for WikiLocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "wiki_locked: another process is writing this wiki scope; retry later"
        )
    }
}
impl std::error::Error for WikiLocked {}

impl ScopeLock {
    pub async fn acquire(root: &Path) -> Result<Self> {
        tokio::fs::create_dir_all(root).await?;
        let dir = root.join(".lock");
        for _ in 0..LOCK_RETRIES {
            match tokio::fs::create_dir(&dir).await {
                Ok(()) => {
                    let owner = format!("{{\"pid\":{}}}", std::process::id());
                    let _ = tokio::fs::write(dir.join("owner.json"), owner).await;
                    return Ok(Self { dir });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(&dir).await {
                        // 回收陈旧锁：只删锁目录本身，删除失败（被他人抢先回收）则继续重试。
                        let _ = tokio::fs::remove_dir_all(&dir).await;
                        continue;
                    }
                    tokio::time::sleep(LOCK_RETRY_DELAY).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
        bail!(WikiLocked)
    }
}

async fn is_stale(dir: &Path) -> bool {
    let Ok(meta) = tokio::fs::metadata(dir).await else {
        return false;
    };
    meta.modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age > STALE_LOCK)
}

impl Drop for ScopeLock {
    fn drop(&mut self) {
        // Drop 不能 await；锁目录很小，同步删除即可。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
