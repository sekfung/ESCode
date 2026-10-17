//! Host 侧任务索引 syncer（S3b，docs/specs/rust-u2-tasks-index.md §6.7；TS 真相源
//! `packages/services/src/escode-agent/escodeTaskIndexSyncer.ts`）。
//!
//! 对每个出现过运行时的 workspace，向 sessions-index / workspace-config 两个 topic 建立
//! **被动订阅**（`existing-only`，绝不拉起运行时），把 V4 topic 帧落成 tasks-index 行与
//! workspace 事件——缺少这一层时，索引里的任务标题与状态在会话创建后会逐渐过时。
//!
//! 状态所有者：本模块是 V4 摄入状态（订阅代际/水位/基线摘要/恢复态）的唯一所有者；
//! 索引写经与其他域相同的 `ReadContext` 句柄（单写者不变式），广播经同一 `WorkspaceEvents`
//! 总线。生命周期驱动：`available` → ensure + 订阅；`unavailable` → 仅清本地订阅态
//! （CLI 内订阅已随进程销毁，不发清理 RPC）；换代 available → 重订阅。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;

use crate::agent_v4::base::Base;
use crate::domains_task_support::ReadContext;
use crate::workspace_events::WorkspaceEvents;

mod apply;
mod apply_writes;
mod topic;
mod topic_subscribe;

/// 摘要变化信号的容量：`window-controller` 的投影刷新是合并式的（脏标记），
/// 慢消费者只可能丢中间信号、不可能因为 lag 停摆（`RecvError::Lagged` 照样继续读最新基线）。
const OVERLAY_CHANGE_CAPACITY: usize = 64;

/// 进程内唯一实例（`channels()` 装配时 spawn，随进程存活；无 dispose 面）。
pub(crate) struct TaskIndexSyncer {
    ingests: Mutex<BTreeMap<String, Arc<Ingest>>>,
    /// 会话摘要基线变化（`window-controller` 的 overlay 刷新触发；消费方按身份键过滤）。
    overlay_changes: tokio::sync::broadcast::Sender<String>,
    /// 手机 replayable 命令队列：phase 终态时收口当前 input 并放行下一条。
    commands: Arc<crate::task_commands::TaskCommandQueue>,
}

/// 一个 workspace 的摄入状态（TS `WorkspaceIngestState` 的拆分）。
struct Ingest {
    pub base: Arc<Base>,
    /// 索引句柄（缺 `dataBaseDir` 时为 None：syncer 只在能写索引时启动，见 `channels()`）。
    pub read: ReadContext,
    pub events: Arc<WorkspaceEvents>,
    /// Host 进程内命令队列（终端迁移 → 收口 running 命令 → 放行下一条）。
    pub commands: Arc<crate::task_commands::TaskCommandQueue>,
    /// TS `v4ConnectionIdFor("task-index")`：`host-<uuid>#task-index`，进程内稳定。
    pub connection_id: String,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub key: String,
    /// 当前已 attach 的运行时代际；None = dormant（等下一次 available）。
    pub runtime_generation: Mutex<Option<u64>>,
    /// 会话摘要基线 + 是否已播种子（diff 的依据；sessions-index topic 专用）。
    pub core: Mutex<Core>,
    pub index: Mutex<topic::TopicState>,
    pub config: Mutex<topic::TopicState>,
    /// 摘要基线变化信号（`window-controller` 投影刷新；见 [`TaskIndexSyncer::overlay_changes`]）。
    pub overlay_changes: tokio::sync::broadcast::Sender<String>,
    /// 索引写的串行队列：按帧到达序执行（见 [`Ingest::enqueue`]；worker 首次入队时启动）。
    writes: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<WriteJob>>,
}

/// 一次索引写（含回源读）：按入队序串行执行。
type WriteJob = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

struct Core {
    pub summaries: BTreeMap<String, Value>,
    pub seeded: bool,
}

/// `Ingest` 的 workspace 三元组（`key` = 身份键；从 `LifecycleEvent` 与测试夹具两处构造）。
pub(super) struct Workspace {
    pub path: String,
    pub identity: Option<String>,
    pub key: String,
}

impl Ingest {
    fn new(
        base: Arc<Base>,
        read: ReadContext,
        events: Arc<WorkspaceEvents>,
        commands: Arc<crate::task_commands::TaskCommandQueue>,
        connection_id: String,
        workspace: Workspace,
        overlay_changes: tokio::sync::broadcast::Sender<String>,
    ) -> Self {
        let index = topic::TopicState::new(
            crate::agent_v4::frames::TopicKind::SessionsIndex,
            format!("sessions-index/{}", workspace.key),
            apply::index_frame_check,
        );
        let config = topic::TopicState::new(
            crate::agent_v4::frames::TopicKind::WorkspaceConfig,
            format!("workspace-config/{}", workspace.key),
            apply::workspace_config_frame_check,
        );
        Self {
            writes: std::sync::OnceLock::new(),
            base,
            read,
            events,
            commands,
            connection_id,
            workspace_path: workspace.path,
            workspace_identity: workspace.identity,
            key: workspace.key,
            runtime_generation: Mutex::new(None),
            core: Mutex::new(Core {
                summaries: BTreeMap::new(),
                seeded: false,
            }),
            index: Mutex::new(index),
            config: Mutex::new(config),
            overlay_changes,
        }
    }

    /// 摘要基线变化 → 通知 `window-controller`（同一 workspace 的投影据此刷新 overlay）。
    pub(super) fn notify_overlays(&self) {
        // 无接收者时 `send` 返回 Err（正常：没有 controller 订阅）。
        let _ = self.overlay_changes.send(self.key.clone());
    }

    pub(super) fn state(
        &self,
        kind: crate::agent_v4::frames::TopicKind,
    ) -> MutexGuard<'_, topic::TopicState> {
        match kind {
            crate::agent_v4::frames::TopicKind::SessionsIndex => lock(&self.index),
            _ => lock(&self.config),
        }
    }

    /// 入队一次索引写（同一 workspace 的写严格按入队序执行）。
    ///
    /// 修复（评审 R2）：帧应用原先对每条摘要各 `tokio::spawn` 一次写，连续两次改标题、或标题与终态
    /// 补丁可能乱序落库（索引停在旧标题；回归用例实测 3 次里 2 次复现）。TS 的写在帧序同步发起、经
    /// `TaskIndexRepo.enqueueWrite` 串行；这里用单 worker 队列保持同一 workspace 的写按帧到达序执行。
    pub(super) fn enqueue(&self, job: impl std::future::Future<Output = ()> + Send + 'static) {
        let writes = self.writes.get_or_init(|| {
            let (writes, mut jobs) = tokio::sync::mpsc::unbounded_channel::<WriteJob>();
            tokio::spawn(async move {
                while let Some(job) = jobs.recv().await {
                    job.await;
                }
            });
            writes
        });
        let _ = writes.send(Box::pin(job));
    }

    /// 运行时转发目标（TS `...state.target`）。
    pub(super) fn target_json(&self) -> Value {
        let mut target = serde_json::json!({"workspacePath": self.workspace_path});
        if let Some(identity) = &self.workspace_identity {
            target["workspaceIdentity"] = Value::from(identity.clone());
        }
        target
    }
}

impl TaskIndexSyncer {
    /// 组合根入口：spawn 生命周期监听（实例表可用性驱动订阅），返回句柄供
    /// `window-controller` 读取会话摘要（overlay 源）并订阅变化信号。
    pub(crate) fn spawn(
        base: Arc<Base>,
        read: ReadContext,
        events: Arc<WorkspaceEvents>,
        commands: Arc<crate::task_commands::TaskCommandQueue>,
    ) -> Arc<Self> {
        let (overlay_changes, _) = tokio::sync::broadcast::channel(OVERLAY_CHANGE_CAPACITY);
        let syncer = Arc::new(Self {
            ingests: Mutex::new(BTreeMap::new()),
            overlay_changes,
            commands,
        });
        let mut lifecycle = base.instances.lifecycle();
        // 任务即所有者：syncer 与监听同生命周期（进程内单例；`weak` 会在 spawn 返回后立即失效）。
        let listener = Arc::clone(&syncer);
        tokio::spawn(async move {
            while let Some(event) = lifecycle.recv().await {
                match event.state {
                    escode_host_runtime::RuntimeState::Available => {
                        listener.runtime_available(&base, &read, &events, &event);
                    }
                    escode_host_runtime::RuntimeState::Unavailable => {
                        listener.runtime_unavailable(&event);
                    }
                }
            }
        });
        syncer
    }

    /// 测试用：无摄入、无生命周期监听的空实例（`window-controller` 单测只用到摘要源与变化信号）。
    #[cfg(test)]
    pub(crate) fn empty_for_test(_events: Arc<WorkspaceEvents>) -> Self {
        let (overlay_changes, _) = tokio::sync::broadcast::channel(OVERLAY_CHANGE_CAPACITY);
        Self {
            ingests: Mutex::new(BTreeMap::new()),
            overlay_changes,
            commands: Arc::new(crate::task_commands::TaskCommandQueue::new()),
        }
    }

    /// 会话摘要基线（`window-controller` 的 overlay 源；未登记的 workspace 回空）。
    pub(crate) fn summaries(&self, identity: &str) -> Vec<Value> {
        let ingest = lock(&self.ingests).get(identity).cloned();
        let Some(ingest) = ingest else {
            return Vec::new();
        };
        lock(&ingest.core).summaries.values().cloned().collect()
    }

    /// 摘要变化信号（广播；消费方按身份键过滤，合并式刷新，丢中间信号无副作用）。
    pub(crate) fn overlay_changes(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.overlay_changes.subscribe()
    }

    fn runtime_available(
        &self,
        base: &Arc<Base>,
        read: &ReadContext,
        events: &Arc<WorkspaceEvents>,
        event: &escode_host_runtime::LifecycleEvent,
    ) {
        let Some(ingest) = self.ensure(base, read, events, event) else {
            return;
        };
        {
            let mut generation = lock(&ingest.runtime_generation);
            if *generation == Some(event.generation) {
                return;
            }
            *generation = Some(event.generation);
        }
        // 换代重订（CLI 内订阅已失）与首次建立同形：两个 topic 各自 fresh subscribe。
        let ingest = Arc::clone(&ingest);
        tokio::spawn(async move {
            topic_subscribe::subscribe_topic(
                &ingest,
                crate::agent_v4::frames::TopicKind::SessionsIndex,
                false,
            )
            .await;
            topic_subscribe::subscribe_topic(
                &ingest,
                crate::agent_v4::frames::TopicKind::WorkspaceConfig,
                false,
            )
            .await;
        });
    }

    fn runtime_unavailable(&self, event: &escode_host_runtime::LifecycleEvent) {
        let Some(ingest) = lock(&self.ingests).get(&event.identity).cloned() else {
            return;
        };
        {
            let mut generation = lock(&ingest.runtime_generation);
            if *generation != Some(event.generation) {
                return;
            }
            *generation = None;
        }
        topic::suspend(&ingest);
    }

    /// TS `ensureWorkspaceSubscription`：幂等建 ingest；帧监听必须先于任何订阅安装。
    fn ensure(
        &self,
        base: &Arc<Base>,
        read: &ReadContext,
        events: &Arc<WorkspaceEvents>,
        event: &escode_host_runtime::LifecycleEvent,
    ) -> Option<Arc<Ingest>> {
        if event.workspace_path.is_empty() {
            return None;
        }
        let mut ingests = lock(&self.ingests);
        if let Some(existing) = ingests.get(&event.identity) {
            return Some(Arc::clone(existing));
        }
        let ingest = Arc::new(Ingest::new(
            Arc::clone(base),
            read.clone(),
            Arc::clone(events),
            Arc::clone(&self.commands),
            format!("host-{}#task-index", uuid::Uuid::new_v4()),
            Workspace {
                path: event.workspace_path.clone(),
                identity: event
                    .workspace_identity
                    .clone()
                    .filter(|value| !value.is_empty()),
                key: event.identity.clone(),
            },
            self.overlay_changes.clone(),
        ));
        topic::install_listeners(&ingest);
        ingests.insert(event.identity.clone(), Arc::clone(&ingest));
        Some(ingest)
    }
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// TS `Date.now()`。
pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "task_index_syncer_order_tests.rs"]
mod order_tests;
#[cfg(test)]
#[path = "task_index_syncer_projection_tests.rs"]
mod projection_tests;
#[cfg(test)]
#[path = "task_index_syncer_queue_tests.rs"]
mod queue_tests;
#[cfg(test)]
#[path = "task_index_syncer_tests.rs"]
mod tests;
