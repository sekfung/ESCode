//! 窗口控制器的刷新引擎（TS `windowHostControllerService.ts` 的 source 聚合与刷新）：
//! 索引读 + 会话摘要叠加 → 投影；变更信号（索引写事件 / 摘要变化）只置脏标记并按 source 串行续跑。
//! 帧构造与状态见 `projection.rs` / `rows.rs`。

use escode_host_control::logging;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use escode_host_services::tasks_index::query::TaskFilters;
use serde_json::{Value, json};

use super::overlays;
use super::projection::Projection;
use super::rows::{LocalScope, row_key, source_key_of};
use crate::domains_task_state::StateContext;
use crate::domains_task_support::{GLM_PROVIDER, ReadContext};
use crate::session::RpcError;
use crate::task_index_syncer::TaskIndexSyncer;
use crate::workspace_events::WorkspaceEvents;

/// 每 source 的“有变更信号尚未被读库消化”标记。
///
/// 协议（保证信号既不丢也不无限循环）：读库**之前**才 `begin_read` 清标记，因此读库期间到达的信号会留下标记、
/// 触发续跑；读库**失败**时 `restore` 把标记放回去（否则这次信号被吞掉，投影会一直旧到下一次信号）。
#[derive(Default)]
pub(super) struct DirtyFlags(Mutex<BTreeMap<String, bool>>);

impl DirtyFlags {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, bool>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn mark(&self, key: &str) {
        self.lock().insert(key.to_owned(), true);
    }

    pub(super) fn is_dirty(&self, key: &str) -> bool {
        self.lock().contains_key(key)
    }

    pub(super) fn begin_read(&self, key: &str) {
        self.lock().remove(key);
    }

    pub(super) fn restore(&self, key: &str) {
        self.mark(key);
    }
}

/// Host 级共享句柄（进程内唯一；连接作用域只持订阅表）。
pub(crate) struct WindowController {
    projector: Arc<Projection>,
    read: ReadContext,
    events: Arc<WorkspaceEvents>,
    syncer: Arc<TaskIndexSyncer>,
    /// 每 source 的刷新串行锁（TS single-flight：并发刷新等待同一次读库）。
    refresh_locks: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 刷新期间到达的变更信号（在飞读结束后续跑一轮；不用超时掩蔽同步问题）。
    dirty: DirtyFlags,
    /// 每 source 一次的变更监听（WorkspaceEvents 总线，登记后不再重建）。
    watchers: Mutex<BTreeMap<String, WatcherTask>>,
    /// syncer 摘要变化监听（与 `watchers` 同类的存续期长子任务）。
    overlay_watcher: Mutex<Option<WatcherTask>>,
}

/// 一个存续期与 controller 同长的监听任务：持有 `JoinHandle` 即持有任务本体，`abort` 供 Drop 取消
/// （`JoinHandle` 不可 Clone，取消句柄单独留一份）。
struct WatcherTask {
    _join: tokio::task::JoinHandle<()>,
    abort: tokio::task::AbortHandle,
}

impl WatcherTask {
    fn new(join: tokio::task::JoinHandle<()>) -> Self {
        Self {
            abort: join.abort_handle(),
            _join: join,
        }
    }
}

impl WindowController {
    pub(crate) fn new(
        read: ReadContext,
        events: Arc<WorkspaceEvents>,
        syncer: Arc<TaskIndexSyncer>,
    ) -> Arc<Self> {
        let controller = Arc::new(Self {
            projector: Arc::new(Projection::new(Box::new(|| {
                uuid::Uuid::new_v4().to_string()
            }))),
            read,
            events,
            syncer,
            refresh_locks: Mutex::new(BTreeMap::new()),
            dirty: DirtyFlags::default(),
            watchers: Mutex::new(BTreeMap::new()),
            overlay_watcher: Mutex::new(None),
        });
        controller.watch_overlay_changes();
        controller
    }

    pub(crate) fn projector(&self) -> &Arc<Projection> {
        &self.projector
    }

    /// 索引读句柄（搜索分支直查 DB；与 `escode-task` 共用同一份懒打开的索引）。
    pub(super) fn read(&self) -> &ReadContext {
        &self.read
    }

    /// 测试探针：当前登记的监听任务取消句柄（drop 后轮询 `is_finished` 断言任务真的停表）。
    #[cfg(test)]
    pub(crate) fn watcher_abort_handles(&self) -> Vec<tokio::task::AbortHandle> {
        let mut handles: Vec<_> = self
            .watchers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|task| task.abort.clone())
            .collect();
        if let Some(task) = self
            .overlay_watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            handles.push(task.abort.clone());
        }
        handles
    }

    /// 写面上下文（`escode-task` 的同一份索引 + 事件总线，单写者不变式）。
    pub(super) fn state_context(&self) -> StateContext {
        StateContext {
            read: self.read.clone(),
            events: Arc::clone(&self.events),
        }
    }

    /// 变更信号②：syncer 会话摘要变化（按身份键定位已登记的 source）。
    fn watch_overlay_changes(self: &Arc<Self>) {
        let mut changes = self.syncer.overlay_changes();
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            loop {
                match changes.recv().await {
                    Ok(identity) => {
                        let Some(controller) = weak.upgrade() else {
                            return;
                        };
                        controller.signal_by_identity(&identity);
                    }
                    // 合并式刷新：丢中间信号无副作用（下一次信号或列表查询照样收敛）。
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        *self
            .overlay_watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(WatcherTask::new(task));
    }

    /// 变更信号②：syncer 会话摘要变化 → 只替换 overlay（TS `replaceSourceSessionOverlays`，
    /// 不重读索引、不发 removals）。
    fn signal_by_identity(self: &Arc<Self>, identity: &str) {
        let key = source_key_of(identity);
        let Some((scope, _)) = self.projector.source_snapshot(&key) else {
            return;
        };
        if !self.projector.refreshed(&key) {
            // 尚无快照的 source：overlay 只会在下次完整刷新时一起生效（TS 同：先 register 才有行）。
            return;
        }
        let overlays = self.overlays(&scope);
        self.projector.replace_source_overlays(&scope, overlays);
    }

    /// 变更信号①：`WorkspaceEvents` 索引写事件（与 `escode-task` 写面/ syncer 同一条总线）。
    fn ensure_source(self: &Arc<Self>, scope: &LocalScope) {
        self.projector.register_source(scope);
        let key = scope.source_key();
        let mut watchers = self
            .watchers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if watchers.contains_key(&key) {
            return;
        }
        let mut target = json!({"workspacePath": scope.workspace_path});
        if let Some(identity) = &scope.workspace_identity {
            target["workspaceIdentity"] = Value::from(identity.clone());
        }
        let mut stream = self.events.listen(&target);
        let weak = Arc::downgrade(self);
        let watched = scope.clone();
        let task = tokio::spawn(async move {
            use futures_util::StreamExt;
            while stream.next().await.is_some() {
                let Some(controller) = weak.upgrade() else {
                    return;
                };
                controller.signal(&watched);
            }
        });
        watchers.insert(key, WatcherTask::new(task));
    }

    /// 置脏 + 后台刷新（信号路径；调用方不等待）。
    fn signal(self: &Arc<Self>, scope: &LocalScope) {
        self.dirty.mark(&scope.source_key());
        let weak = Arc::downgrade(self);
        let scope = scope.clone();
        tokio::spawn(async move {
            let Some(controller) = weak.upgrade() else {
                return;
            };
            if let Err(error) = controller.refresh(&scope, false).await {
                // 一个 source 的瞬时读取失败不能清空可信投影（TS `onSourceError` 只记录）。
                logging::warn(
                    "window-controller",
                    format!(
                        "refresh failed for {}: {}",
                        scope.workspace_path, error.message
                    ),
                );
            }
        });
    }

    /// TS `refreshSource`：single-flight + 脏标记续跑（覆盖"刷新期间到达的信号"）。
    pub(crate) async fn refresh(
        self: &Arc<Self>,
        scope: &LocalScope,
        force: bool,
    ) -> Result<(), RpcError> {
        let key = scope.source_key();
        self.ensure_source(scope);
        let lock = {
            let mut locks = self
                .refresh_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(locks.entry(key.clone()).or_default())
        };
        let _held = lock.lock().await;
        if !force && !self.dirty.is_dirty(&key) && self.projector.refreshed(&key) {
            return Ok(());
        }
        loop {
            self.dirty.begin_read(&key);
            let memberships = match self.read_memberships(scope).await {
                Ok(memberships) => memberships,
                Err(error) => {
                    // 读库失败：保留“仍需刷新”，下一次列表查询/信号会重试，而不是静默吞掉这次变更。
                    self.dirty.restore(&key);
                    return Err(error);
                }
            };
            let overlays = self.overlays(scope);
            self.projector
                .replace_source_snapshot(scope, memberships, overlays);
            if !self.dirty.is_dirty(&key) {
                return Ok(());
            }
        }
    }

    /// TS `readSourceTaskIndex`：active/pinned/archived 三读合并（后读覆盖先读），
    /// meta 的 workspace 字段按 scope 归一（TS `normalizeTaskMeta`）。
    async fn read_memberships(
        &self,
        scope: &LocalScope,
    ) -> Result<BTreeMap<String, Value>, RpcError> {
        let read = self.read.clone();
        let path = scope.workspace_path.clone();
        let identity = scope.workspace_identity.clone();
        read.run(move |index| {
            let mut merged: BTreeMap<String, Value> = BTreeMap::new();
            for (pinned, archived, flags) in [
                (Some(false), Some(false), (false, false, true)),
                (Some(true), Some(false), (true, false, true)),
                (None, Some(true), (false, true, false)),
            ] {
                let filters = TaskFilters {
                    workspace_path: Some(path.as_str()),
                    workspace_identity: identity.as_deref(),
                    provider: Some(GLM_PROVIDER),
                    pinned,
                    archived,
                    include_deleted: false,
                };
                let metas = index
                    .list_task_metas(&filters)
                    .map_err(|error| error.to_string())?;
                for mut meta in metas {
                    let task_id = meta["taskId"].as_str().unwrap_or_default().to_owned();
                    normalize_scope(&mut meta, &LocalScope::new(&path, identity.as_deref()));
                    let (pinned, archived, active) = flags;
                    merged.insert(
                        row_key(&path, identity.as_deref(), &task_id),
                        json!({
                            "meta": meta,
                            "membership": {
                                "pinned": pinned,
                                "archived": archived,
                                "active": active,
                            },
                        }),
                    );
                }
            }
            Ok(merged)
        })
        .await
        .map_err(RpcError::from)
    }

    /// 会话摘要 → overlay（syncer 的基线是唯一源；`existing-only`，列表读取不启动运行时）。
    fn overlays(&self, scope: &LocalScope) -> BTreeMap<String, Value> {
        let identity = scope
            .workspace_identity
            .clone()
            .unwrap_or_else(|| scope.workspace_path.clone());
        let mut overlays = BTreeMap::new();
        for summary in self.syncer.summaries(&identity) {
            let overlay = overlays::session_overlay(&summary);
            if let Some(task_id) = overlay["taskId"].as_str() {
                overlays.insert(task_id.to_owned(), overlay);
            }
        }
        overlays
    }
}

/// 取消路径：两个监听任务（每 source 的索引写监听 + syncer 摘要监听）都靠 `Weak` 判活，
/// **drop 后收尾不是确定的**——owner 先释放时任务可能还挂在 `recv()` 上等下一次事件。
/// controller 的存续期所有者就是 `channels()` 装配出的 channel 表，表没了监听就该停：
/// 在 Drop 里 abort，避免任务被连带泄漏。
impl Drop for WindowController {
    fn drop(&mut self) {
        let watchers = std::mem::take(
            &mut *self
                .watchers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (_, task) in watchers {
            task.abort.abort();
        }
        if let Some(task) = self
            .overlay_watcher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort.abort();
        }
    }
}

/// TS `normalizeTaskMeta`：`workspacePath`/`workspaceIdentity` 覆盖为 scope 的值（缺 identity 时删键）。
pub(crate) fn normalize_scope(meta: &mut Value, scope: &LocalScope) {
    let Some(record) = meta.as_object_mut() else {
        return;
    };
    record.insert(
        "workspacePath".to_owned(),
        Value::from(scope.workspace_path.clone()),
    );
    match &scope.workspace_identity {
        Some(identity) => {
            record.insert(
                "workspaceIdentity".to_owned(),
                Value::from(identity.clone()),
            );
        }
        None => {
            record.remove("workspaceIdentity");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{DirtyFlags, LocalScope, WindowController};
    use crate::domains_task_support::{LazyIndex, ReadContext};
    use crate::task_index_syncer::TaskIndexSyncer;
    use crate::workspace_events::WorkspaceEvents;

    /// 测试用索引：库文件 + 迁移账本（`TasksIndex::open` 只校验不建库）。
    fn prepare_index(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(path).unwrap();
        for statement in escode_host_services::tasks_index::schema::MIGRATED_DDL {
            connection.execute_batch(statement).unwrap();
        }
        for (id, checksum) in escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS {
            connection
                .execute(
                    "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
                    rusqlite::params![id, checksum],
                )
                .unwrap();
        }
    }

    fn controller_fixture(index_path: std::path::PathBuf) -> Arc<WindowController> {
        let read = ReadContext {
            path: index_path,
            index: Arc::new(std::sync::Mutex::new(LazyIndex::default())),
            targets: Arc::new(crate::task_targets::TaskTargets::new()),
            runtime: None,
        };
        let events = Arc::new(WorkspaceEvents::default());
        let syncer = Arc::new(TaskIndexSyncer::empty_for_test(Arc::clone(&events)));
        WindowController::new(read, events, syncer)
    }

    /// 取消路径（评审遗留）：owner 释放后两个监听任务必须真的停表，而不是继续挂在
    /// `recv()` 上等下一次事件（`Weak` 只在事件到达时才判活，不是取消）。
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_controller_aborts_its_watcher_tasks() {
        let temp = tempfile::tempdir().unwrap();
        let index_path = temp.path().join("tasks-index.sqlite");
        let prepare_path = index_path.clone();
        tokio::task::spawn_blocking(move || prepare_index(&prepare_path))
            .await
            .unwrap();
        let controller = controller_fixture(index_path);
        let scope = LocalScope::new("/workspace/alpha", None);
        controller.refresh(&scope, true).await.unwrap();
        // 每 source 的索引写监听 + syncer 摘要监听。
        let tasks = controller.watcher_abort_handles();
        assert_eq!(tasks.len(), 2, "注册 source 后应有两个监听任务");

        drop(controller);

        // abort 是协作式的：让出后任务才会看到取消信号。
        let settled = tokio::time::timeout(Duration::from_secs(2), async {
            while !tasks.iter().all(tokio::task::AbortHandle::is_finished) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(settled.is_ok(), "drop 后监听任务必须收尾：{tasks:?}");
    }

    #[test]
    fn signals_during_a_read_survive_and_failed_reads_keep_the_flag() {
        let flags = DirtyFlags::default();
        flags.mark("s");
        assert!(flags.is_dirty("s"));
        flags.begin_read("s");
        assert!(!flags.is_dirty("s"), "读库前清标记");
        flags.mark("s");
        assert!(flags.is_dirty("s"), "读库期间到达的信号留下标记 → 续跑");
        flags.begin_read("s");
        flags.restore("s");
        assert!(flags.is_dirty("s"), "读库失败 → 放回标记，下次重试");
        assert!(!flags.is_dirty("other"));
    }
}
