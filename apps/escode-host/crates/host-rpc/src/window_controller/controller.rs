//! 窗口控制器的刷新引擎（TS `windowHostControllerService.ts` 的 source 聚合与刷新）：
//! 索引读 + 会话摘要叠加 → 投影；变更信号（索引写事件 / 摘要变化）只置脏标记并按 source 串行续跑。
//! 帧构造与状态见 `projection.rs` / `rows.rs`。

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

/// Host 级共享句柄（进程内唯一；连接作用域只持订阅表）。
pub(crate) struct WindowController {
    projector: Arc<Projection>,
    read: ReadContext,
    events: Arc<WorkspaceEvents>,
    syncer: Arc<TaskIndexSyncer>,
    /// 每 source 的刷新串行锁（TS single-flight：并发刷新等待同一次读库）。
    refresh_locks: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 刷新期间到达的变更信号（在飞读结束后续跑一轮；不用超时掩蔽同步问题）。
    dirty: Mutex<BTreeMap<String, bool>>,
    /// 每 source 一次的变更监听（WorkspaceEvents 总线，登记后不再重建）。
    watchers: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
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
            dirty: Mutex::new(BTreeMap::new()),
            watchers: Mutex::new(BTreeMap::new()),
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

    /// 写面上下文（`escode-task` 的同一份索引 + 事件总线，单写者不变式）。
    pub(super) fn state_context(&self) -> StateContext {
        StateContext {
            read: self.read.clone(),
            events: Arc::clone(&self.events),
        }
    }

    fn lock_dirty(&self) -> MutexGuard<'_, BTreeMap<String, bool>> {
        self.dirty
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 变更信号②：syncer 会话摘要变化（按身份键定位已登记的 source）。
    fn watch_overlay_changes(self: &Arc<Self>) {
        let mut changes = self.syncer.overlay_changes();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
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
        watchers.insert(key, task);
    }

    /// 置脏 + 后台刷新（信号路径；调用方不等待）。
    fn signal(self: &Arc<Self>, scope: &LocalScope) {
        self.lock_dirty().insert(scope.source_key(), true);
        let weak = Arc::downgrade(self);
        let scope = scope.clone();
        tokio::spawn(async move {
            let Some(controller) = weak.upgrade() else {
                return;
            };
            if let Err(error) = controller.refresh(&scope, false).await {
                // 一个 source 的瞬时读取失败不能清空可信投影（TS `onSourceError` 只记录）。
                eprintln!(
                    "escode-host: window-controller refresh failed for {}: {}",
                    scope.workspace_path, error.message
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
        let mut dirty = self.lock_dirty().remove(&key).unwrap_or(false);
        if !force && !dirty && self.projector.refreshed(&key) {
            return Ok(());
        }
        loop {
            let memberships = self.read_memberships(scope).await?;
            let overlays = self.overlays(scope);
            self.projector
                .replace_source_snapshot(scope, memberships, overlays);
            dirty = self.lock_dirty().remove(&key).unwrap_or(false);
            if !dirty {
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
