//! `file-watcher` 域（docs/specs/rust-u2-file-watcher.md；TS 真相源 `packages/services/src/fileWatcher/`）。
//!
//! Host 内唯一的 [`FileWatcherService`]：watcher 属于 Host 而不属于连接。每个 watcher = 一个 `notify` 句柄 +
//! 一个防抖 actor（TS：每次变更重置 150 ms 定时器，到点投递一次）。

mod events;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher as _};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use events::{Change, map_event};

/// TS `DEBOUNCE_MS`。
const DEBOUNCE: Duration = Duration::from_millis(150);

type Listeners = Arc<Mutex<Vec<mpsc::UnboundedSender<Value>>>>;
type Table = Arc<Mutex<HashMap<String, Watcher>>>;

struct Watcher {
    /// 持有即保活；drop 即关闭底层句柄。
    _notify: Box<dyn notify::Watcher + Send>,
    listeners: Listeners,
    cancel: watch::Sender<bool>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Host 内唯一的 file-watcher 服务。
pub struct FileWatcherService {
    next_id: AtomicU64,
    watchers: Table,
    debounce: Duration,
}

impl Default for FileWatcherService {
    fn default() -> Self {
        Self::with_debounce(DEBOUNCE)
    }
}

impl FileWatcherService {
    pub fn new() -> Self {
        Self::default()
    }

    /// 测试用：缩短防抖窗口。
    pub fn with_debounce(debounce: Duration) -> Self {
        Self {
            next_id: AtomicU64::new(0),
            watchers: Arc::default(),
            debounce,
        }
    }

    /// TS `watch`。失败文案：`无法监视目录 '<path>': <底层错误>`。
    pub async fn watch(&self, path: &str, recursive: bool) -> anyhow::Result<String> {
        // TS 先领号再建 watcher：失败也消耗 id。
        let id = self.next_id.fetch_add(1, Ordering::SeqCst).to_string();
        let fail = |message: String| anyhow::anyhow!("无法监视目录 '{path}': {message}");
        if let Err(error) = tokio::fs::metadata(path).await {
            // 文案对齐 Node `fs.watch` 的 libuv 错误。
            return Err(fail(match error.kind() {
                std::io::ErrorKind::NotFound => {
                    format!("ENOENT: no such file or directory, watch '{path}'")
                }
                std::io::ErrorKind::PermissionDenied => {
                    format!("EACCES: permission denied, watch '{path}'")
                }
                _ => error.to_string(),
            }));
        }

        let (change_tx, change_rx) = mpsc::unbounded_channel::<Change>();
        let root = path.to_owned();
        let mut native = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    for change in map_event(&root, recursive, &event) {
                        let _ = change_tx.send(change);
                    }
                }
                Err(error) => {
                    let _ = change_tx.send(Change::Error(error.to_string()));
                }
            },
        )
        .map_err(|error| fail(error.to_string()))?;
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        native
            .watch(Path::new(path), mode)
            .map_err(|error| fail(error.to_string()))?;

        let listeners: Listeners = Arc::default();
        let (cancel, cancelled) = watch::channel(false);
        tokio::spawn(run_actor(Actor {
            id: id.clone(),
            path: path.to_owned(),
            debounce: self.debounce,
            changes: change_rx,
            listeners: Arc::clone(&listeners),
            cancel: cancelled,
            table: Arc::clone(&self.watchers),
        }));
        lock(&self.watchers).insert(
            id.clone(),
            Watcher {
                _notify: Box::new(native),
                listeners,
                cancel,
            },
        );
        Ok(id)
    }

    /// TS `unwatch`（`cleanup`）：未知 id 静默成功。
    pub fn unwatch(&self, id: &str) {
        cleanup(&self.watchers, id);
    }

    /// TS `disposeAll`。
    pub fn dispose_all(&self) {
        let ids: Vec<String> = lock(&self.watchers).keys().cloned().collect();
        for id in ids {
            cleanup(&self.watchers, &id);
        }
    }

    /// TS `onDynamicChange`：未知/已失效 id 返回 `None`（调用方记 warn、给空流），不报错。
    pub fn listen(&self, id: &str) -> Option<mpsc::UnboundedReceiver<Value>> {
        let table = lock(&self.watchers);
        let watcher = table.get(id)?;
        let (tx, rx) = mpsc::unbounded_channel();
        lock(&watcher.listeners).push(tx);
        Some(rx)
    }
}

fn cleanup(table: &Table, id: &str) {
    let Some(watcher) = lock(table).remove(id) else {
        return;
    };
    let _ = watcher.cancel.send(true);
    lock(&watcher.listeners).clear();
}

struct Actor {
    id: String,
    path: String,
    debounce: Duration,
    changes: mpsc::UnboundedReceiver<Change>,
    listeners: Listeners,
    cancel: watch::Receiver<bool>,
    table: Table,
}

fn fire(listeners: &Listeners, event: &Value) {
    lock(listeners).retain(|tx| tx.send(event.clone()).is_ok());
}

async fn run_actor(mut actor: Actor) {
    let mut pending: BTreeSet<String> = BTreeSet::new();
    let mut unknown = false;
    let mut deadline: Option<Instant> = None;
    loop {
        tokio::select! {
            biased;
            _ = actor.cancel.changed() => return,
            change = actor.changes.recv() => match change {
                None => return,
                Some(Change::Path(path)) => {
                    pending.insert(path);
                    deadline = Some(Instant::now() + actor.debounce);
                }
                Some(Change::Unknown) => {
                    unknown = true;
                    deadline = Some(Instant::now() + actor.debounce);
                }
                Some(Change::Error(message)) => {
                    eprintln!(
                        "escode-host: file watcher error, cleaning up id={} path={} error={message}",
                        actor.id, actor.path
                    );
                    // TS：发一个仅含 dirPath 的最终事件，再清理。
                    fire(&actor.listeners, &json!({"dirPath": actor.path}));
                    cleanup(&actor.table, &actor.id);
                    return;
                }
            },
            () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                deadline = None;
                // 同一防抖窗口只含一个明确路径时才透传，避免过滤掉同批次里的目标文件事件。
                let only = (!unknown && pending.len() == 1).then(|| pending.iter().next().cloned()).flatten();
                pending.clear();
                unknown = false;
                let mut event = json!({"dirPath": actor.path});
                if let Some(only) = only {
                    event["changedPath"] = Value::from(only);
                }
                fire(&actor.listeners, &event);
            }
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
