//! 文件指纹与变更闸门（rust-unified-host.md P1 #7；语义对标 ZCode-rs
//! `rust/crates/kit/src/fs/stamp.rs`）。
//!
//! 轮询方（provider 配置每秒刷新）先用 [`FileStamp::of`] 取指纹、经 [`ChangeGate`]
//! 判断是否需要真读：指纹未变且未到兜底全读间隔就跳过读取与哈希。指纹覆盖长度、
//! mtime，Unix 另加 (dev, ino) 与 ctime——inode 抓住「同长度同 mtime 的原子替换写」，
//! ctime 工具改不回去；Windows 没有 inode/ctime，靠 `full_read_every` 兜底。
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// `path` 在某一时刻的元数据指纹。`meta: None` 表示元数据不可读（不存在或权限不足）；
/// 之后变得可读时指纹随之改变，触发一次真读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    path: PathBuf,
    meta: Option<Meta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Meta {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64),
    #[cfg(unix)]
    changed: (i64, i64),
}

impl FileStamp {
    /// 取指纹（跟随符号链接）。
    pub async fn of(path: &Path) -> Self {
        let meta = match tokio::fs::metadata(path).await {
            Ok(meta) => Some(meta_of(&meta)),
            Err(_) => None,
        };
        Self {
            path: path.to_owned(),
            meta,
        }
    }
}

fn meta_of(meta: &std::fs::Metadata) -> Meta {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Meta {
            len: meta.len(),
            modified: meta.modified().ok(),
            inode: (meta.dev(), meta.ino()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
    #[cfg(not(unix))]
    {
        Meta {
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

/// 真读闸门：`full_read_every` 是指纹不变时两次读取之间的最长间隔（兜底）。
/// 只给单一轮询循环使用，不共享、不加锁（调用方负责串行）。
#[derive(Debug)]
pub struct ChangeGate {
    full_read_every: Duration,
    last: Option<(Vec<FileStamp>, Instant)>,
}

impl ChangeGate {
    pub fn new(full_read_every: Duration) -> Self {
        Self {
            full_read_every,
            last: None,
        }
    }

    /// 是否需要真读：从未成功读过、指纹（含路径与顺序）与上次不同、或兜底间隔已到。
    pub fn should_read(&self, stamps: &[FileStamp], now: Instant) -> bool {
        match &self.last {
            None => true,
            Some((last, at)) => last != stamps || now.duration_since(*at) >= self.full_read_every,
        }
    }

    /// 记录「本次读取前取的指纹」。读取失败或结果要重试时不调用，下一轮自然真读
    /// ——行为等同于完全没有闸门。
    pub fn record(&mut self, stamps: Vec<FileStamp>, now: Instant) {
        self.last = Some((stamps, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("fixture write");
    }

    /// 原子替换（temp + rename）。同长度内容替换时只有 inode/ctime 会变。
    fn atomic_replace(path: &Path, bytes: &[u8]) {
        let temp = path.with_extension("tmp-stage");
        write(&temp, bytes);
        std::fs::rename(&temp, path).expect("atomic rename");
    }

    #[tokio::test]
    async fn appearance_changes_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let missing = FileStamp::of(&path).await;
        write(&path, b"{\"a\":1}");
        let present = FileStamp::of(&path).await;
        assert_ne!(missing, present, "出现必须改变指纹");
        assert_eq!(present, FileStamp::of(&path).await, "未动时指纹稳定");
    }

    #[tokio::test]
    async fn rewrite_and_delete_change_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write(&path, b"{\"a\":1}");
        let before = FileStamp::of(&path).await;
        write(&path, b"{\"a\":1,\"b\":2}");
        let after = FileStamp::of(&path).await;
        assert_ne!(before, after, "内容长度变化必须改变指纹");
        std::fs::remove_file(&path).unwrap();
        assert_ne!(after, FileStamp::of(&path).await, "删除必须改变指纹");
    }

    // 修复：只在 Unix 断言。Windows 没有 inode/ctime，按模块文档由 `full_read_every` 兜底
    // （gate_skips_until_change_or_backstop 覆盖）；同一时间刻度内的同长度替换 len 与 mtime 都相同，
    // 指纹本就不变（windows-latest test 作业暴露）。
    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_replace_of_equal_length_changes_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write(&path, b"{\"a\":1111}");
        let before = FileStamp::of(&path).await;
        // 同长度替换：inode 变，ctime 变。
        atomic_replace(&path, b"{\"a\":2222}");
        let after = FileStamp::of(&path).await;
        assert_ne!(before, after, "同长度原子替换必须改变指纹");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn in_place_write_on_same_inode_is_caught_by_ctime_or_backstop() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write(&path, b"{\"a\":1111}");
        let before = FileStamp::of(&path).await;
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"{\"a\":2222}").unwrap();
        file.sync_all().unwrap();
        let after = FileStamp::of(&path).await;
        // 同 inode 原地写：ctime 必然前进（mtime/长度可能不变）。
        assert_ne!(before, after, "原地写必须改变指纹（ctime）");
    }

    #[test]
    fn gate_skips_until_change_or_backstop() {
        let now = Instant::now();
        let mut gate = ChangeGate::new(Duration::from_secs(60));
        let stamps = vec![stamp_fixture("a", 1, 10)];
        assert!(gate.should_read(&stamps, now), "首次必须读");
        gate.record(stamps.clone(), now);
        assert!(!gate.should_read(&stamps, now + Duration::from_secs(59)));
        assert!(
            gate.should_read(&stamps, now + Duration::from_secs(60)),
            "兜底间隔到点必须读"
        );
        let changed = vec![stamp_fixture("a", 2, 10)];
        assert!(gate.should_read(&changed, now), "指纹变化必须读");
        let reordered = vec![stamp_fixture("a", 1, 10), stamp_fixture("b", 1, 10)];
        assert!(
            gate.should_read(&reordered, now),
            "集合（含顺序）变化必须读"
        );
    }

    #[tokio::test]
    async fn account_bypass_still_reads_files_after_a_recorded_round() {
        // 账号旁路（refresh(Some(account))）不走闸门：闸门只描述文件指纹，
        // 调用方在账号事实变化时必须真读（等价于 record 未被调用前的行为）。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        write(&path, b"{}");
        let now = Instant::now();
        let mut gate = ChangeGate::new(Duration::from_secs(60));
        let stamps = vec![FileStamp::of(&path).await];
        gate.record(stamps.clone(), now);
        assert!(!gate.should_read(&stamps, now + Duration::from_secs(1)));
        // 账号更新后调用方跳过闸门直接读，并重新记录（此处只验证 record 的更新时间语义）。
        gate.record(stamps.clone(), now + Duration::from_secs(1));
        assert!(!gate.should_read(&stamps, now + Duration::from_secs(2)));
        assert!(gate.should_read(&stamps, now + Duration::from_secs(61)));
    }

    fn stamp_fixture(name: &str, len: u64, mtime_secs: u64) -> FileStamp {
        FileStamp {
            path: PathBuf::from(format!("/fixture/{name}")),
            meta: Some(Meta {
                len,
                modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs)),
                #[cfg(unix)]
                inode: (1, len),
                #[cfg(unix)]
                changed: (mtime_secs as i64, 0),
            }),
        }
    }
}
