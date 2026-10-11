//! 会话的文件读取状态（TS `readFileState`）：读后再写的新鲜度校验，以及重复 Read 的去重
//! （docs/specs/rust-file-tool-results.md）。键均为 realpath 后的路径。
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// 观察缓存与范围缓存都有界；淘汰只会要求重新 Read，不会跳过新鲜度检查。
const LIMIT: usize = 1024;

#[derive(Default, Clone)]
pub struct FileState {
    entries: HashMap<PathBuf, Observation>,
    ranges: HashMap<(PathBuf, u64, Option<u64>), Stamp>,
}

#[derive(Clone)]
pub(super) struct Observation {
    pub hash: Vec<u8>,
    pub full: bool,
}

/// TS `isCachedReadFresh` 比较的文件戳：整数毫秒 mtime 与字节数。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    mtime_ms: u128,
    size: u64,
}

impl Stamp {
    pub(super) async fn of(path: &Path) -> Option<Self> {
        let meta = tokio::fs::metadata(path).await.ok()?;
        let mtime_ms = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis();
        Some(Self { mtime_ms, size: meta.len() })
    }
}

impl FileState {
    /// 记为已读取（MEMORY.md 已注入上下文、Bash 读文件回填等，TS 写入 readFileState）。
    pub(super) fn observe(&mut self, path: PathBuf, bytes: &[u8], full: bool) {
        self.remember(path, Sha256::digest(bytes).to_vec(), full);
    }
    pub(super) fn remember(&mut self, path: PathBuf, hash: Vec<u8>, full: bool) {
        if self.entries.len() >= LIMIT && !self.entries.contains_key(&path) {
            self.entries.clear();
        }
        self.entries.insert(path, Observation { hash, full });
    }
    pub(super) fn observation(&self, path: &Path) -> Option<&Observation> {
        self.entries.get(path)
    }
    pub(super) fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        self.entries.keys().cloned().collect()
    }
    /// TS `createReadFileStateKey(path, offset ?? 1, limit)` 的一次非 partial view 读取（Read、Write/Edit 后、Bash 回填）。
    pub(super) fn record_range(&mut self, path: PathBuf, offset: u64, limit: Option<u64>, stamp: Stamp) {
        if self.ranges.len() >= LIMIT {
            self.ranges.clear();
        }
        self.ranges.insert((path, offset, limit), stamp);
    }
    /// TS `isCachedReadFresh`：同一路径与范围的上次读取之后文件未变（mtime 与大小都相同）。
    pub(super) fn range_fresh(&self, path: &Path, offset: u64, limit: Option<u64>, stamp: Stamp) -> bool {
        self.ranges.get(&(path.to_owned(), offset, limit)) == Some(&stamp)
    }
}
