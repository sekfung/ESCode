//! WikiService：绑定一个 workspace 的 Wiki 服务（spec §2）。项目身份在首次使用时解析并固定；
//! 只能打开 global 与本项目两个存储——没有任何入口能按名字打开其他项目（规则 2）。
use super::identity::resolve_project;
use super::settings::{Settings, load};
use super::store::{Loaded, ScopeStore};
use anyhow::Result;
use escode_cli_domain::wiki::entry::{Entry, Scope};
use escode_cli_domain::wiki::index::Index;
use escode_cli_domain::wiki::project::ProjectIdentity;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{Mutex, OnceCell};

/// 手工编辑条目文件（编辑器原地写入不改目录 mtime）最多这么久后被重新校验。
const SNAPSHOT_TTL: Duration = Duration::from_secs(60);

/// 一个 scope 的只读快照：条目 + 索引。由目录 mtime 与索引文件状态失效。
#[derive(Clone)]
pub struct Snapshot {
    pub store: ScopeStore,
    pub entries: Arc<HashMap<String, Entry>>,
    pub index: Arc<Index>,
    pub invalid: Arc<Vec<(String, String)>>,
    stamp: Stamp,
    loaded_at: Instant,
}

#[derive(Clone, PartialEq)]
struct Stamp {
    entries_dir: Option<SystemTime>,
    index_file: Option<(SystemTime, u64)>,
}

pub struct WikiService {
    pub(crate) cwd: PathBuf,
    pub(crate) workspace_identity: Option<String>,
    pub(crate) home: PathBuf,
    project: OnceCell<ProjectIdentity>,
    snapshots: Mutex<HashMap<PathBuf, Snapshot>>,
    pub(crate) turn_cache: Mutex<super::retrieval::TurnCache>,
}

impl WikiService {
    pub fn new(cwd: PathBuf, workspace_identity: Option<String>, home: PathBuf) -> Self {
        Self {
            cwd,
            workspace_identity,
            home,
            project: OnceCell::new(),
            snapshots: Mutex::new(HashMap::new()),
            turn_cache: Mutex::new(Default::default()),
        }
    }

    pub async fn settings(&self) -> Settings {
        load(&self.cwd, &self.home).await
    }

    pub async fn project(&self) -> &ProjectIdentity {
        self.project
            .get_or_init(|| resolve_project(&self.cwd, self.workspace_identity.as_deref()))
            .await
    }

    /// global 与当前项目的存储（顺序固定：project 在前，同 ID 时项目知识优先展示）。
    pub async fn stores(&self, settings: &Settings) -> (ScopeStore, ScopeStore) {
        let project = self.project().await;
        (
            ScopeStore::project(&settings.root, &project.id),
            ScopeStore::global(&settings.root),
        )
    }

    pub async fn store_for(&self, settings: &Settings, scope: Scope) -> ScopeStore {
        let (project, global) = self.stores(settings).await;
        if scope == Scope::Global {
            global
        } else {
            project
        }
    }

    async fn stamp(store: &ScopeStore) -> Stamp {
        let entries_dir = tokio::fs::metadata(store.root.join("entries"))
            .await
            .ok()
            .and_then(|m| m.modified().ok());
        let index_file = tokio::fs::metadata(store.root.join("index.json"))
            .await
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        Stamp {
            entries_dir,
            index_file,
        }
    }

    /// 读快照（只读路径不落盘重建的索引，避免与写锁竞争；写路径会重建并落盘）。
    pub async fn snapshot(&self, store: &ScopeStore) -> Result<Snapshot> {
        let stamp = Self::stamp(store).await;
        if let Some(cached) = self.snapshots.lock().await.get(&store.root)
            && cached.stamp == stamp
            && cached.loaded_at.elapsed() < SNAPSHOT_TTL
        {
            return Ok(cached.clone());
        }
        let Loaded { entries, invalid } = store.load().await?;
        let (index, _) = store.index(&entries, false).await?;
        let snapshot = Snapshot {
            store: store.clone(),
            entries: Arc::new(
                entries
                    .into_iter()
                    .map(|e| (e.meta.id.clone(), e))
                    .collect(),
            ),
            index: Arc::new(index),
            invalid: Arc::new(invalid),
            stamp,
            loaded_at: Instant::now(),
        };
        self.snapshots
            .lock()
            .await
            .insert(store.root.clone(), snapshot.clone());
        Ok(snapshot)
    }

    /// 写操作后调用：丢弃快照与检索缓存。
    pub async fn invalidate(&self) {
        self.snapshots.lock().await.clear();
        self.turn_cache.lock().await.clear();
    }

    /// 来源路径展示：项目根内为相对路径，其余为绝对路径（规范分隔符）。
    pub async fn display_path(&self, absolute: &Path) -> String {
        let root = PathBuf::from(&self.project().await.root);
        match absolute.strip_prefix(&root) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => absolute.to_string_lossy().replace('\\', "/"),
        }
    }
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn age_days(timestamp: &str) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some((chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_days())
}
