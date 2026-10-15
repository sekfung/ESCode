//! `file` 域的工作区文件索引与搜索（docs/specs/rust-u2-file-index.md；TS 真相源 `packages/services/src/file/`）。
//!
//! 进程内状态只有索引缓存（60 s TTL、最多 4 项、LRU，键为 workspaceKey，签名含 `.escodeignore` 指纹）。

mod codec;
mod ignore_rules;
mod scan;
mod search;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::OnceCell;

use codec::IndexEntry;
use search::Candidate;

pub use crate::git::LocaleCompare;

const CACHE_TTL: Duration = Duration::from_secs(60);
const CACHE_MAX_ENTRIES: usize = 4;

struct Index {
    at: Instant,
    signature: String,
    packed: String,
    /// `packed.length`（UTF-16 码元）。
    packed_len: usize,
    units: OnceLock<Vec<u16>>,
    entries: Vec<IndexEntry>,
    candidates: OnceLock<Vec<Candidate>>,
}

type Scan = Arc<OnceCell<Result<Arc<Index>, String>>>;

#[derive(Default)]
struct State {
    /// 最旧在前（LRU）。
    cache: Vec<(String, Arc<Index>)>,
    scanning: HashMap<String, (String, Scan)>,
}

pub struct FileIndexService {
    state: Mutex<State>,
    locale_compare: LocaleCompare,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// TS `statWorkspaceFileSearchIgnoreFingerprint`：`mtimeMs:size`，不存在为 `none`。
async fn ignore_fingerprint(root: &str) -> String {
    match tokio::fs::metadata(Path::new(root).join(ignore_rules::IGNORE_FILE_NAME)).await {
        Ok(info) => {
            let mtime = info
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{mtime}:{}", info.len())
        }
        Err(_) => "none".to_owned(),
    }
}

impl FileIndexService {
    pub fn new(locale_compare: LocaleCompare) -> Self {
        Self {
            state: Mutex::default(),
            locale_compare,
        }
    }

    /// TS `ensureWorkspaceFileIndex`。
    async fn ensure(
        &self,
        root: &str,
        identity: Option<&str>,
        refresh: bool,
    ) -> anyhow::Result<Arc<Index>> {
        let key = identity
            .map(crate::git::paths::js_trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(root)
            .to_owned();
        let rules = Arc::new(ignore_rules::load_rules(root).await);
        let signature = format!("{root}\n{}", ignore_fingerprint(root).await);
        // 同步决定（持锁段内不 await）：命中缓存 / 加入在途扫描 / 发起新扫描。
        enum Plan {
            Hit(Arc<Index>),
            Join(Scan),
            Start(Scan),
        }
        let plan = {
            let mut state = lock(&self.state);
            state
                .cache
                .retain(|(_, index)| index.at.elapsed() < CACHE_TTL);
            let joinable = (!refresh)
                .then(|| state.scanning.get(&key))
                .flatten()
                .filter(|(sig, _)| *sig == signature)
                .map(|(_, cell)| Arc::clone(cell));
            if let Some(cell) = joinable {
                Plan::Join(cell)
            } else if let Some(position) = (!refresh)
                .then(|| {
                    state
                        .cache
                        .iter()
                        .position(|(k, index)| *k == key && index.signature == signature)
                })
                .flatten()
            {
                // 命中后移到队尾（LRU）。
                let entry = state.cache.remove(position);
                let index = Arc::clone(&entry.1);
                state.cache.push(entry);
                Plan::Hit(index)
            } else {
                let cell: Scan = Arc::default();
                state
                    .scanning
                    .insert(key.clone(), (signature.clone(), Arc::clone(&cell)));
                Plan::Start(cell)
            }
        };
        let cell = match plan {
            Plan::Hit(index) => return Ok(index),
            // 同签名的在途扫描：所有等待者用同一个初始化闭包，谁先到谁执行，其余等同一个结果。
            Plan::Join(cell) => {
                return run_scan(&cell, root, rules, &self.locale_compare, signature)
                    .await
                    .map_err(anyhow::Error::msg);
            }
            Plan::Start(cell) => cell,
        };
        let outcome = run_scan(&cell, root, rules, &self.locale_compare, signature).await;
        {
            let mut state = lock(&self.state);
            let ours = state
                .scanning
                .get(&key)
                .is_some_and(|(_, current)| Arc::ptr_eq(current, &cell));
            if ours {
                state.scanning.remove(&key);
                // 显式刷新/规则变化可替换在途扫描：被替换的旧扫描只服务旧请求，不覆盖新索引。
                if let Ok(index) = &outcome {
                    state.cache.retain(|(k, _)| *k != key);
                    state.cache.push((key, Arc::clone(index)));
                    while state.cache.len() > CACHE_MAX_ENTRIES {
                        state.cache.remove(0);
                    }
                }
            }
        }
        outcome.map_err(anyhow::Error::msg)
    }

    /// TS `searchWorkspaceFiles`。`limit` 来自 JS number（可为 NaN/Infinity，由调用方转成 `None` 表示非法）。
    pub async fn search(
        &self,
        root: &str,
        identity: Option<&str>,
        query: &str,
        limit: Option<f64>,
        refresh: bool,
    ) -> anyhow::Result<Value> {
        let requested = limit.unwrap_or(search::DISPLAY_CAP as f64);
        if !requested.is_finite() {
            anyhow::bail!("Invalid workspace file search query or limit");
        }
        let limit = requested.trunc().clamp(0.0, search::DISPLAY_CAP as f64) as usize;
        if limit == 0 {
            return Ok(json!([]));
        }
        let index = self.ensure(root, identity, refresh).await?;
        let root_owned = root.to_owned();
        let query_owned = query.to_owned();
        let hits = tokio::task::spawn_blocking(move || {
            let candidates = index.candidates.get_or_init(|| {
                let (prefix, separator) = codec::root_prefix(&root_owned);
                index
                    .entries
                    .iter()
                    .map(|entry| {
                        let (name, path) =
                            codec::name_and_path(&entry.relative_path, &prefix, separator);
                        Candidate::new(name, path, entry.relative_path.clone(), entry.directory)
                    })
                    .collect()
            });
            search::top_k(candidates, &query_owned, limit)
                .into_iter()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "path": c.path,
                        "relativePath": c.relative_path,
                        "type": if c.directory { "directory" } else { "file" },
                    })
                })
                .collect::<Vec<_>>()
        })
        .await?;
        Ok(Value::Array(hits))
    }

    /// TS `listWorkspaceFilesLength`（不传 identity、不 refresh）。
    pub async fn length(&self, root: &str) -> anyhow::Result<usize> {
        Ok(self.ensure(root, None, false).await?.packed_len)
    }

    /// TS `listWorkspaceFilesRange`：`[offset, offset+length)`（UTF-16 码元）。
    pub async fn range(&self, root: &str, offset: f64, length: f64) -> anyhow::Result<String> {
        let index = self.ensure(root, None, false).await?;
        let offset = offset.trunc().max(0.0);
        if offset.is_nan() || offset >= index.packed_len as f64 {
            return Ok(String::new());
        }
        let length = if length.is_nan() {
            0.0
        } else {
            length.trunc().max(0.0)
        };
        let start = offset as usize;
        let end = (offset + length).min(index.packed_len as f64) as usize;
        let units = index
            .units
            .get_or_init(|| index.packed.encode_utf16().collect());
        Ok(String::from_utf16_lossy(&units[start..end]))
    }

    pub async fn read_ignore(&self, root: &str) -> anyhow::Result<Value> {
        let (content, source) = ignore_rules::read_for_settings(root).await?;
        Ok(json!({"content": content, "source": source}))
    }

    pub async fn transform_ignore(&self, root: &str, transform: &str) -> anyhow::Result<Value> {
        Ok(json!({"content": ignore_rules::transform(root, transform).await?}))
    }

    pub async fn write_ignore(&self, root: &str, content: &str) -> anyhow::Result<()> {
        Ok(ignore_rules::write_atomic(root, content).await?)
    }
}

async fn run_scan(
    cell: &Scan,
    root: &str,
    rules: Arc<ignore_rules::Rules>,
    compare: &LocaleCompare,
    signature: String,
) -> Result<Arc<Index>, String> {
    let (root, compare) = (root.to_owned(), Arc::clone(compare));
    cell.get_or_init(|| async move {
        tokio::task::spawn_blocking(move || build_index(&root, &rules, &compare, signature))
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map(Arc::new).map_err(|error| error.to_string()))
    })
    .await
    .clone()
}

/// 扫描 + 排序 + 打包（阻塞线程）。
fn build_index(
    root: &str,
    rules: &ignore_rules::Rules,
    compare: &LocaleCompare,
    signature: String,
) -> anyhow::Result<Index> {
    let mut entries = scan::walk(root, rules)?;
    entries.sort_by(|left, right| {
        if left.directory != right.directory {
            return if left.directory {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        compare(&left.relative_path, &right.relative_path)
    });
    let packed = codec::pack(&entries);
    let packed_len = packed.encode_utf16().count();
    Ok(Index {
        at: Instant::now(),
        signature,
        packed,
        packed_len,
        units: OnceLock::new(),
        entries,
        candidates: OnceLock::new(),
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
