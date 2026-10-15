//! `window-controller` 的 Host 侧列表投影（TS `windowHostControllerProjection.ts` 的**本地子集**）。
//!
//! 纯状态机：只持有 source（rows/membership/overlay，见 [`super::rows`]）/ topic 水位
//! （`logEpoch`+`seq`）/ 订阅表，**不做 IO**——索引读由 `WindowController::refresh` 完成后调用
//! [`Projection::replace_source_snapshot`]。帧构造逐字对齐 TS 投影（子 spec §2/§3）：
//! 快照帧 `fromSeq:0`、每批增量 `seq+=1`、overlay 替换只发 upserted。

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};

use super::frames::{
    ack, publish_task_deltas, publish_workspace_deltas, snapshot_frame, sorted_rows, topic_name,
    topic_state,
};
pub(super) use super::rows::{LocalScope, Source};
use super::rows::{build_rows, workspace_fact};
use crate::session::RpcError;

pub(super) const TASKS_TOPIC: &str = "controller/tasks-index";
pub(super) const WORKSPACES_TOPIC: &str = "controller/workspaces";

/// 帧出口：往连接作用域投递一次 `EventFire`（订阅号 → 连接监听者/早期缓冲）。
pub(crate) type FrameSink = std::sync::Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Clone)]
pub(super) struct TopicState {
    pub(super) log_epoch: String,
    pub(super) seq: u64,
}

pub(super) struct Subscriber {
    pub(super) topic: &'static str,
    pub(super) sink: FrameSink,
}

pub(super) struct State {
    pub(super) sources: BTreeMap<String, Source>,
    pub(super) subscribers: BTreeMap<String, Subscriber>,
    pub(super) tasks: TopicState,
    pub(super) workspaces: TopicState,
}

pub(crate) struct Projection {
    new_id: Box<dyn Fn() -> String + Send + Sync>,
    state: Mutex<State>,
}

impl Projection {
    pub(crate) fn new(new_id: Box<dyn Fn() -> String + Send + Sync>) -> Self {
        // TS 在投影构造时即为两个 topic 生成 `logEpoch`（随机值，仅需进程内唯一）。
        Self {
            state: Mutex::new(State {
                sources: BTreeMap::new(),
                subscribers: BTreeMap::new(),
                tasks: TopicState {
                    log_epoch: new_id(),
                    seq: 0,
                },
                workspaces: TopicState {
                    log_epoch: new_id(),
                    seq: 0,
                },
            }),
            new_id,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn id(&self) -> String {
        (self.new_id)()
    }

    /// TS `registerSource`：幂等登记（本地 source 初始 `offline`/`connecting`，首个快照转 online）。
    pub(crate) fn register_source(&self, scope: &LocalScope) {
        let mut state = self.lock();
        state
            .sources
            .entry(scope.source_key())
            .or_insert_with(|| Source {
                scope: scope.clone(),
                online: false,
                connection_state: "connecting",
                memberships: BTreeMap::new(),
                overlays: BTreeMap::new(),
                rows: BTreeMap::new(),
                refreshed: false,
            });
    }

    /// 已成功快照过一次（非强制刷新可跳过读库）。
    pub(crate) fn refreshed(&self, source_key: &str) -> bool {
        self.lock()
            .sources
            .get(source_key)
            .is_some_and(|source| source.refreshed)
    }

    /// 已登记的本地 source（`(scope, refreshed)`）；刷新器据此决定是否读库。
    pub(crate) fn source_snapshot(&self, source_key: &str) -> Option<(LocalScope, bool)> {
        self.lock()
            .sources
            .get(source_key)
            .map(|source| (source.scope.clone(), source.refreshed))
    }

    /// TS `replaceSourceSnapshot`：membership + overlay 一并替换，发布 removals + upserted。
    pub(crate) fn replace_source_snapshot(
        &self,
        scope: &LocalScope,
        memberships: BTreeMap<String, Value>,
        overlays: BTreeMap<String, Value>,
    ) {
        let mut state = self.lock();
        let (deltas, previous_fact, next_fact) = {
            let Some(source) = state.sources.get_mut(&scope.source_key()) else {
                return;
            };
            let previous_rows = std::mem::take(&mut source.rows);
            let previous_fact = workspace_fact(source);
            source.memberships = memberships;
            source.overlays = overlays;
            source.online = true;
            source.connection_state = "online";
            source.refreshed = true;
            let next_rows = build_rows(source);
            source.rows = next_rows.clone();
            let next_fact = workspace_fact(source);

            let mut deltas = Vec::new();
            for (key, previous) in &previous_rows {
                if !next_rows.contains_key(key) {
                    deltas.push(json!({"op": "task.removed", "address": previous["address"]}));
                }
            }
            for (key, task) in &next_rows {
                if previous_rows.get(key) != Some(task) {
                    deltas.push(json!({"op": "task.upserted", "task": task}));
                }
            }
            (deltas, previous_fact, next_fact)
        };
        publish_task_deltas(&mut state, deltas);
        if previous_fact != next_fact {
            publish_workspace_deltas(
                &mut state,
                vec![json!({"op": "workspace.upserted", "workspace": next_fact})],
            );
        }
    }

    /// TS `replaceSourceSessionOverlays`：只重建行并发布 upserted，**不发 removal**。
    pub(crate) fn replace_source_overlays(
        &self,
        scope: &LocalScope,
        overlays: BTreeMap<String, Value>,
    ) {
        let mut state = self.lock();
        let deltas = {
            let Some(source) = state.sources.get_mut(&scope.source_key()) else {
                return;
            };
            let previous_rows = std::mem::take(&mut source.rows);
            source.overlays = overlays;
            let next_rows = build_rows(source);
            source.rows = next_rows.clone();
            let mut deltas = Vec::new();
            for (key, task) in &next_rows {
                if previous_rows.get(key) != Some(task) {
                    deltas.push(json!({"op": "task.upserted", "task": task}));
                }
            }
            deltas
        };
        publish_task_deltas(&mut state, deltas);
    }

    /// 行快照（WC-2 列表查询；`source_keys` 为空表示全部 source）。
    pub(crate) fn rows(&self, source_keys: &[String]) -> Vec<Value> {
        let state = self.lock();
        sorted_rows(&state, source_keys)
    }

    /// 单行查询（搜索结果叠加与 mutation 返回值）。
    pub(crate) fn row(&self, address: &Value, task_id: &str) -> Option<Value> {
        let state = self.lock();
        let scope = scope_of_address(address);
        state
            .sources
            .get(&scope.source_key())
            .and_then(|source| source.rows.get(&scope.row_key(task_id)))
            .cloned()
    }

    /// TS `projection.mutate` 的校验部分：地址必须命中唯一 source；除归档删除外行必须存在；
    /// source 必须在线（文案与 TS 同）。
    pub(crate) fn validate_mutation(
        &self,
        address: &Value,
        allow_missing_task: bool,
    ) -> Result<LocalScope, RpcError> {
        let state = self.lock();
        // 远程 source 本片不支持（子 spec §5-1）：remoteSessionId 地址不会命中任何 source。
        if address["remoteSessionId"].is_string() {
            return Err(RpcError::new("Error", "没有与任务地址匹配的 source"));
        }
        let scope = scope_of_address(address);
        let Some(source) = state.sources.get(&scope.source_key()) else {
            return Err(RpcError::new("Error", "没有与任务地址匹配的 source"));
        };
        let task_id = address["taskId"].as_str().unwrap_or_default();
        if !allow_missing_task && !source.rows.contains_key(&scope.row_key(task_id)) {
            return Err(RpcError::new("Error", "没有与任务地址匹配的 source"));
        }
        if !source.online {
            return Err(RpcError::new("Error", "本地 source 当前不可用"));
        }
        Ok(scope)
    }

    /// `subscribeControllerV4`：登记订阅号 → 立即投递快照帧 → 回 ack。
    pub(crate) fn subscribe(&self, topic: &str, sink: FrameSink) -> Result<Value, RpcError> {
        let topic = topic_name(topic)?;
        let subscription_id = self.id();
        let mut state = self.lock();
        let frame = snapshot_frame(&state, topic, &subscription_id);
        let log_epoch = topic_state(&state, topic).log_epoch.clone();
        sink(frame);
        state
            .subscribers
            .insert(subscription_id.clone(), Subscriber { topic, sink });
        Ok(ack(&subscription_id, &log_epoch))
    }

    /// `resyncControllerV4`：未知订阅号回 TS 同文案错误；命中即重发快照。
    pub(crate) fn resync(&self, subscription_id: &str) -> Result<Value, RpcError> {
        let state = self.lock();
        let Some(subscriber) = state.subscribers.get(subscription_id) else {
            return Err(RpcError::new(
                "Error",
                format!("未找到 Controller subscription，subscriptionId={subscription_id}"),
            ));
        };
        let topic = subscriber.topic;
        let frame = snapshot_frame(&state, topic, subscription_id);
        let log_epoch = topic_state(&state, topic).log_epoch.clone();
        (subscriber.sink)(frame);
        Ok(ack(subscription_id, &log_epoch))
    }

    /// `unsubscribeControllerV4`：未知订阅号是 no-op（TS 同）。
    pub(crate) fn unsubscribe(&self, subscription_id: &str) {
        self.lock().subscribers.remove(subscription_id);
    }

    /// 当前订阅数（测试与诊断）。
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.lock().subscribers.len()
    }

    /// 连接收口：注销该连接登记过的全部订阅号。
    pub(crate) fn unsubscribe_all(&self, subscription_ids: &[String]) {
        let mut state = self.lock();
        for id in subscription_ids {
            state.subscribers.remove(id);
        }
    }
}

fn scope_of_address(address: &Value) -> LocalScope {
    LocalScope::new(
        address["workspacePath"].as_str().unwrap_or_default(),
        address["workspaceIdentity"].as_str(),
    )
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "projection_fixture_tests.rs"]
mod fixture_tests;
