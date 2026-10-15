//! V4 作用域：三类 topic 的订阅生命周期（subscribe → ACK 后登记 → unsubscribe/resync）、连接水位转发与关闭。

use std::sync::Arc;
use std::time::Instant;

use serde_json::{Map, Value, json};

use super::base::{conversation_subscribe_body, index_subscribe_body, with_open_timing};
use super::frames::TopicKind;
use super::scope::{Owned, Scope, fault, flow_key, policy_of, workspace_key, workspace_target};
use crate::forward::{RUNTIME_UNAVAILABLE_CODE, RuntimePolicy, workspace_ref_json};
use crate::session::{FlowState, RpcError};

impl Scope {
    /// `subscribe{Conversation,SessionsIndex,WorkspaceConfig}V4`。
    pub(crate) async fn subscribe(
        &self,
        kind: TopicKind,
        params: &Value,
    ) -> Result<Value, RpcError> {
        let started = Instant::now();
        self.assert_ready()?;
        let connection = self.connection();
        let workspace = workspace_key(params);
        let (topic, ownership_key) = match kind {
            TopicKind::Conversation => {
                let topic = format!(
                    "conversation/{}",
                    params["sessionId"].as_str().unwrap_or_default()
                );
                let ownership =
                    format!("conversation\0{workspace}\0{topic}\0{}", self.connection_id);
                (topic, ownership)
            }
            TopicKind::SessionsIndex | TopicKind::WorkspaceConfig => {
                let topic = format!("{}/{workspace}", kind.as_str());
                let ownership = format!("{}\0{topic}\0{}", kind.as_str(), self.connection_id);
                (topic, ownership)
            }
        };
        let pending = self.begin_pending(kind, params, &topic);
        let outcome = async {
            let (body, policy, task_meta_ms) = match kind {
                TopicKind::Conversation => {
                    let hint_started = Instant::now();
                    let hint = self.base.resume_thought_level(params).await;
                    let hint_ms = u64::try_from(hint_started.elapsed().as_millis()).unwrap_or(0);
                    let workspace_ref = workspace_ref_json(
                        params["workspacePath"].as_str().unwrap_or_default(),
                        params["workspaceIdentity"].as_str(),
                        params["remoteSessionId"].as_str(),
                        &workspace,
                    );
                    let body = conversation_subscribe_body(
                        params,
                        &topic,
                        &connection,
                        workspace_ref,
                        hint,
                    );
                    // 历史 topic 是只读事实源：冷订阅只启动 CLI（TS `getReadOnlyClient(params)`）。
                    (body, RuntimePolicy::StartIfNeeded, Some(hint_ms))
                }
                // 侧栏为全部 restored workspace 登记被动订阅：缺 runtime 时 existing-only 进入 dormant。
                _ => (
                    index_subscribe_body(params, &topic, &connection),
                    policy_of(params),
                    None,
                ),
            };
            let subscribed = self.base.subscribe(params, policy, body, started).await?;
            let result = match task_meta_ms {
                Some(ms) => with_open_timing(subscribed.result, &subscribed.timing, ms),
                None => subscribed.result,
            };
            Ok::<_, RpcError>(result)
        }
        .await;
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                self.discard_pending(pending);
                return Err(error);
            }
        };
        let subscription_id = result["ack"]["subscriptionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let entry = Owned {
            kind,
            ownership_key,
            subscription_id,
            topic,
            workspace_key: workspace,
            target: workspace_target(params),
        };
        match self.remember_after_subscribe(pending, entry.clone()) {
            Ok(()) => {}
            Err((reason, unsubscribe)) => {
                if unsubscribe {
                    let _ = self.unsubscribe_base(&entry, None).await;
                }
                return Err(fault(reason));
            }
        }
        // 当前 transport 已 saturated：订阅 ACK 才让该 route 成为事实，必须在此补发一次 SAT。
        if self.lock().current_flow == Some(FlowState::Saturated)
            && let Err(error) = self
                .forward_flow(
                    &flow_key(&entry.workspace_key, &self.connection_id),
                    &entry.target,
                    FlowState::Saturated,
                )
                .await
        {
            self.forget(&entry);
            let _ = self.unsubscribe_base(&entry, None).await;
            return Err(error);
        }
        Ok(result)
    }

    /// `unsubscribe*V4`：找不到 owner 静默成功。
    pub(crate) async fn unsubscribe(
        &self,
        kind: TopicKind,
        params: &Value,
    ) -> Result<(), RpcError> {
        let subscription_id = params["subscriptionId"].as_str().unwrap_or_default();
        let Some(entry) = self.find(kind, params, subscription_id) else {
            return Ok(());
        };
        self.forget(&entry);
        // 最后一个订阅必须先 closed 再 unsubscribe（反序会让上游先忘 owner、closed 被拒绝后留下 paused 连接）。
        let key = flow_key(&entry.workspace_key, &self.connection_id);
        let still_routed = self.flow_routes().contains_key(&key);
        let forwarded = self.lock().forwarded_flow.contains_key(&key);
        let closed = if !still_routed && forwarded {
            let result = self
                .forward_flow(&key, &entry.target, FlowState::Closed)
                .await;
            self.lock().forwarded_flow.remove(&key);
            result
        } else {
            Ok(())
        };
        let unsubscribed = self
            .unsubscribe_base(&entry, params.get("runtimePolicy").cloned())
            .await;
        closed.and(unsubscribed)
    }

    /// `resync*V4`：必须是本连接拥有的订阅；应答回来时该订阅仍须是当前代（防 stale success）。
    pub(crate) async fn resync(&self, kind: TopicKind, params: &Value) -> Result<Value, RpcError> {
        self.assert_ready()?;
        let subscription_id = params["subscriptionId"].as_str().unwrap_or_default();
        let entry = self
            .find(kind, params, subscription_id)
            .ok_or_else(|| fault("fault.subscription.notOwned"))?;
        let mut body = Map::new();
        body.insert("topic".to_owned(), Value::from(entry.topic.clone()));
        body.insert(
            "connectionId".to_owned(),
            Value::from(self.connection_id.clone()),
        );
        body.insert(
            "subscriptionId".to_owned(),
            Value::from(entry.subscription_id.clone()),
        );
        body.insert(
            "base".to_owned(),
            params.get("base").cloned().unwrap_or(Value::Null),
        );
        if let Some(force) = params.get("forceSnapshot").filter(|value| !value.is_null()) {
            body.insert("forceSnapshot".to_owned(), force.clone());
        }
        if body["base"].is_null() {
            body.remove("base");
        }
        let mut routing = entry.target.clone();
        if let Some(policy) = params.get("runtimePolicy") {
            routing["runtimePolicy"] = policy.clone();
        }
        let called = self
            .base
            .call(
                &routing,
                policy_of(&routing),
                "v4/conversation/resync",
                Value::Object(body),
            )
            .await?;
        if !self.is_owned(&entry) {
            return Err(fault("fault.subscription.notOwned"));
        }
        Ok(called.result)
    }

    pub(super) async fn unsubscribe_base(
        &self,
        entry: &Owned,
        runtime_policy: Option<Value>,
    ) -> Result<(), RpcError> {
        let mut routing = entry.target.clone();
        if let Some(policy) = runtime_policy.filter(|value| !value.is_null()) {
            routing["runtimePolicy"] = policy;
        }
        let body = json!({
            "topic": entry.topic,
            "subscriptionId": entry.subscription_id,
            "connectionId": self.connection_id,
        });
        self.base
            .call(
                &routing,
                policy_of(&routing),
                "v4/conversation/unsubscribe",
                body,
            )
            .await
            .map(|_| ())
    }

    /// 向 runtime 转发一个路由的水位状态（同值去重）。只对已存在的实例发（不为水位拉起运行时）。
    pub(super) async fn forward_flow(
        &self,
        key: &str,
        target: &Value,
        state: FlowState,
    ) -> Result<(), RpcError> {
        if self.lock().forwarded_flow.get(key) == Some(&state) {
            return Ok(());
        }
        let body = json!({"connectionId": self.connection_id, "state": state.as_str()});
        match self
            .base
            .call(
                target,
                RuntimePolicy::ExistingOnly,
                "v4/connection/flow",
                body,
            )
            .await
        {
            Ok(_) => {
                self.lock().forwarded_flow.insert(key.to_owned(), state);
                Ok(())
            }
            // 运行时已不在：其订阅随之消失，没有需要通知的对象。
            Err(error)
                if error.extra.get("code") == Some(&Value::from(RUNTIME_UNAVAILABLE_CODE)) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// 连接水位边沿（TS `setTransportFlowState` → `applyTransportFlowState`），按提交序串行。
    pub(crate) async fn apply_flow(&self, state: FlowState) {
        let _chain = self.flow_chain.lock().await;
        {
            let mut current = self.lock();
            if current.flow_closed && state != FlowState::Closed {
                return;
            }
            current.current_flow = Some(state);
            if state == FlowState::Closed {
                current.flow_closed = true;
            }
        }
        for (key, target) in self.flow_routes() {
            if let Err(error) = self.forward_flow(&key, &target, state).await {
                eprintln!(
                    "escode-host: forwarding connection flow {} failed: {}",
                    state.as_str(),
                    error.message
                );
            }
        }
    }

    /// 连接关闭（TS `dispose`）：先把 closed 排在已入队的边沿之后，再注销全部 owner。
    pub(crate) async fn dispose(self: &Arc<Self>) {
        self.apply_flow(FlowState::Closed).await;
        for task in std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ) {
            task.abort();
        }
        let Some(entries) = self.take_for_dispose() else {
            return;
        };
        let unsubscribes = entries
            .iter()
            .map(|entry| self.unsubscribe_base(entry, None));
        let _ = futures_util::future::join_all(unsubscribes).await;
    }
}
