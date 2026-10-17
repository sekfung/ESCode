//! `gen-ui` 状态存储（TS `gen-ui/adapters/stateStorage.ts`）。
//!
//! 三层串行化各有职责，不能省：
//! 1. **scope 级异步锁**：同一 scope 的「读现有记录 → 取 updatedAt → 写 → 淘汰」必须是一个整体；
//! 2. **跨进程文件锁**（`<scope 目录>/state`，`escode-cli-host::file_lock` 与 TS 同策略）：
//!    双 Host 交替运行或另一个进程写同一目录时仍互斥；
//! 3. **文件级通知串行**：watcher 事件与写入 ACK 可能同时到，串行读保证较早的快照不迟到覆盖较新的通知。
//!
//! 通知去重（`last_published`）与「首次读取建立基线」见 `rust-u2-gen-ui.md` §3.2/§3.3。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use notify::{RecursiveMode, Watcher as _};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use super::paths::{is_record_file_name, record_file, scope_dir, sort_by_updated_desc};
use super::schema::{
    RECORD_MAX_BYTES, STATE_MAX_ENTRIES, Scope, Target, parse_scope, parse_target,
    parse_widget_state, scope_key,
};

/// 一条已落盘的记录（`file` 供淘汰与去重使用，不进 JSON）。
#[derive(Clone)]
pub struct Record {
    pub target: Target,
    pub state: Value,
    pub updated_at: f64,
    pub file: PathBuf,
}

/// TS `target` 的 JSON 形状：`workspaceIdentity` 缺省时**省略键**（zod optional 不补键）。
fn target_json(target: &Target) -> Value {
    let mut object = Map::new();
    object.insert(
        "workspacePath".to_owned(),
        Value::from(target.scope.workspace_path.clone()),
    );
    if let Some(identity) = &target.scope.workspace_identity {
        object.insert(
            "workspaceIdentity".to_owned(),
            Value::from(identity.clone()),
        );
    }
    object.insert(
        "sessionId".to_owned(),
        Value::from(target.scope.session_id.clone()),
    );
    object.insert("path".to_owned(), Value::from(target.path.clone()));
    Value::Object(object)
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as f64)
        .unwrap_or_default()
}

fn lock_for<K: std::hash::Hash + Eq + Clone>(
    map: &Mutex<HashMap<K, Arc<tokio::sync::Mutex<()>>>>,
    key: K,
) -> Arc<tokio::sync::Mutex<()>> {
    let mut guard = map.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(guard.entry(key).or_default())
}

#[derive(Default)]
struct Inner {
    /// 已装 watcher 的 scope 目录（每个目录一个）。
    watched: Mutex<HashSet<PathBuf>>,
    watchers: Mutex<HashMap<PathBuf, notify::RecommendedWatcher>>,
    /// 状态文件 → 该文件对应的 target（事件要带 target，且只发已知文件）。
    targets: Mutex<HashMap<PathBuf, Target>>,
    /// 上一次已发布的状态序列化（通知去重；首次读取建立基线）。
    last_published: Mutex<HashMap<PathBuf, String>>,
    scope_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    file_locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    listeners: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
}

/// Host 进程内唯一的状态存储。
#[derive(Clone)]
pub struct GenUiStateStore {
    root: PathBuf,
    inner: Arc<Inner>,
}

impl GenUiStateStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            inner: Arc::new(Inner::default()),
        }
    }

    /// 订阅状态变更（每个订阅者一条独立流；无 key、无初始快照）。
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock_listeners().push(tx);
        rx
    }

    fn lock_listeners(&self) -> std::sync::MutexGuard<'_, Vec<mpsc::UnboundedSender<Value>>> {
        self.inner
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// TS `readRecord`：文件不存在回 `None`；超过 96 KiB 或 JSON/schema 不合法**抛错**
    /// （调用方不吞：`listState` 会因此整体失败，`publish` 只记警告）。
    async fn read_record(&self, file: &Path) -> Result<Option<Record>, String> {
        let meta = match tokio::fs::metadata(file).await {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{error}")),
        };
        if meta.len() > RECORD_MAX_BYTES {
            return Err("Invalid Gen UI state record size".to_owned());
        }
        let raw = tokio::fs::read_to_string(file)
            .await
            .map_err(|error| format!("{error}"))?;
        let value: Value = serde_json::from_str(&raw).map_err(|error| format!("{error}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| "Expected object".to_owned())?;
        reject_record_keys(object)?;
        let target = parse_target(object.get("target").unwrap_or(&Value::Null))?;
        let state = parse_widget_state(object.get("state").unwrap_or(&Value::Null))?;
        let updated_at = object
            .get("updatedAt")
            .and_then(Value::as_f64)
            .ok_or_else(|| "updatedAt: Expected number".to_owned())?;
        Ok(Some(Record {
            target,
            state,
            updated_at,
            file: file.to_path_buf(),
        }))
    }

    /// TS `records(scope)`：该 scope 目录下合法文件名的记录，target 必须同 scope，按 `updatedAt` 降序。
    /// 任一条读取失败即整体失败（TS `Promise.all` 同）。
    pub async fn records(&self, scope: &Scope) -> Result<Vec<Record>, String> {
        let dir = scope_dir(&self.root, scope);
        let names = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => {
                let mut names = Vec::new();
                let mut entries = entries;
                while let Some(entry) = entries.next_entry().await.map_err(|e| format!("{e}"))? {
                    if let Some(name) = entry.file_name().to_str()
                        && is_record_file_name(name)
                    {
                        names.push(name.to_owned());
                    }
                }
                names
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("{error}")),
        };
        let wanted = scope_key(scope);
        let mut records = Vec::new();
        for name in names {
            let Some(record) = self.read_record(&dir.join(&name)).await? else {
                continue;
            };
            if scope_key(&record.target.scope) == wanted {
                records.push(record);
            }
        }
        sort_by_updated_desc(&mut records, |record| record.updated_at);
        Ok(records)
    }

    /// TS `observe`：建目录 → 登记 target → 该目录首次观察时装一个目录 watcher。
    async fn observe(&self, target: &Target) -> Result<(), String> {
        let dir = scope_dir(&self.root, &target.scope);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| format!("{error}"))?;
        let file = record_file(&self.root, target);
        {
            let mut watched = self
                .inner
                .watched
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if watched.contains(&dir) {
                return Ok(());
            }
            watched.insert(dir.clone());
        }
        {
            let mut targets = self
                .inner
                .targets
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            targets.insert(file, target.clone());
        }
        let handle = tokio::runtime::Handle::current();
        let inner = Arc::clone(&self.inner);
        let root = self.root.clone();
        let watch_dir = dir.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else { return };
                for path in event.paths {
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if !is_record_file_name(name) {
                        continue;
                    }
                    let store = GenUiStateStore {
                        root: root.clone(),
                        inner: Arc::clone(&inner),
                    };
                    handle.spawn(async move {
                        if let Err(error) = store.publish(&path).await {
                            escode_host_control::logging::warn(
                                "gen-ui-state",
                                format!("state notification failed: {error}"),
                            );
                        }
                    });
                }
            })
            .map_err(|error| format!("{error}"))?;
        watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|error| format!("{error}"))?;
        self.inner
            .watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(watch_dir, watcher);
        Ok(())
    }

    /// TS `publish`：按文件串行，读出当前状态后与基线比对，变了才广播。
    async fn publish(&self, file: &Path) -> Result<(), String> {
        let lock = lock_for(&self.inner.file_locks, file.to_path_buf());
        let _guard = lock.lock().await;
        let target = {
            let targets = self
                .inner
                .targets
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            targets.get(file).cloned()
        };
        let Some(target) = target else { return Ok(()) };
        let state = self
            .read_record(file)
            .await?
            .map(|record| record.state)
            .unwrap_or(Value::Null);
        let serialized = serde_json::to_string(&state).unwrap_or_default();
        if self
            .inner
            .last_published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(file)
            == Some(&serialized)
        {
            return Ok(());
        }
        self.inner
            .last_published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file.to_path_buf(), serialized);
        let event = json!({"target": target_json(&target), "state": state});
        self.lock_listeners()
            .retain(|tx| tx.send(event.clone()).is_ok());
        Ok(())
    }

    /// TS `getState`：读状态；**首个读取者建立通知基线**（否则一次读取会把别的页面等着的变更吃掉）。
    pub async fn get(&self, raw: &Value) -> Result<Value, String> {
        let target = parse_target(raw)?;
        self.observe(&target).await?;
        let file = record_file(&self.root, &target);
        let state = self
            .read_record(&file)
            .await?
            .map(|record| record.state)
            .unwrap_or(Value::Null);
        let mut published = self
            .inner
            .last_published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        published
            .entry(file)
            .or_insert_with(|| serde_json::to_string(&state).unwrap_or_default());
        Ok(state)
    }

    /// TS `setState`。`raw` = `{target, state}`。
    pub async fn set(&self, raw: &Value) -> Result<(), String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "Expected object".to_owned())?;
        let target = parse_target(object.get("target").unwrap_or(&Value::Null))?;
        let state = parse_widget_state(object.get("state").unwrap_or(&Value::Null))?;
        let key = scope_key(&target.scope);
        let lock = lock_for(&self.inner.scope_locks, key);
        let _guard = lock.lock().await;

        self.observe(&target).await?;
        let dir = scope_dir(&self.root, &target.scope);
        // 双 Host / 多进程写同一目录时靠这把跨进程锁互斥（策略与 TS 完全一致）。
        let file_lock = escode_cli_host::file_lock::acquire(&dir.join("state"))
            .await
            .map_err(|error| format!("{error:#}"))?;
        let outcome = self.write(&target, state).await;
        file_lock.release().await;
        outcome?;
        self.publish(&record_file(&self.root, &target)).await
    }

    async fn write(&self, target: &Target, state: Value) -> Result<(), String> {
        let existing = self.records(&target.scope).await?;
        // 同毫秒连写也要单调：取现有最大值 + 1 与当前时刻的较大者。
        let newest = existing.first().map_or(0.0, |record| record.updated_at);
        let updated_at = now_ms().max(newest + 1.0);
        let file = record_file(&self.root, target);
        let payload = serde_json::to_vec(&json!({
            "target": target_json(target),
            "state": state,
            "updatedAt": updated_at,
        }))
        .map_err(|error| format!("{error}"))?;
        let write_path = file.clone();
        tokio::task::spawn_blocking(move || {
            escode_host_core::atomic_write::write(&write_path, &payload)
        })
        .await
        .map_err(|error| format!("{error}"))?
        .map_err(|error| format!("{error:#}"))?;

        // 容量淘汰：除刚写的这条外，按 updatedAt 降序保留 `MAX_ENTRIES - 1` 条。
        for record in existing
            .iter()
            .filter(|record| record.file != file)
            .skip(STATE_MAX_ENTRIES - 1)
        {
            let _ = tokio::fs::remove_file(&record.file).await;
        }
        Ok(())
    }

    /// TS `listState`：该 scope 的记录（已按 updatedAt 降序）投影成 `{path, state}`。
    pub async fn list(&self, raw: &Value) -> Result<Vec<Value>, String> {
        let scope = parse_scope(raw)?;
        Ok(self
            .records(&scope)
            .await?
            .into_iter()
            .take(STATE_MAX_ENTRIES)
            .map(|record| json!({"path": record.target.path, "state": record.state}))
            .collect())
    }
}

/// 记录是 **strict** 的三键（TS `recordSchema.strict()`）。
fn reject_record_keys(object: &Map<String, Value>) -> Result<(), String> {
    if let Some(extra) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "target" | "state" | "updatedAt"))
    {
        return Err(format!("Unrecognized key: \"{extra}\""));
    }
    Ok(())
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
