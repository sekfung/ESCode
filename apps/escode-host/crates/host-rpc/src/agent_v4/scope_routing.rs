//! V4 作用域的帧路由：`onDynamic*Frame` 事件流、上游帧的归属判定 / 暂存 / 投递。
//! 判定与投递在作用域状态锁内完成（与 ACK 后释放暂存帧互斥），保证不乱序。

use std::sync::{Arc, Weak};

use serde_json::Value;
use tokio::sync::mpsc;

use super::frames::{RoutedFrame, TopicKind};
use super::scope::{MAX_PENDING_BYTES, MAX_PENDING_FRAMES, Route, Scope, workspace_key};
use crate::session::EventStream;
use crate::wire::Wire;

impl Scope {
    /// `onDynamic{Conversation,SessionsIndex,WorkspaceConfig}Frame(target)`：同 (kind, workspace) 共用一条上游。
    pub(crate) fn routed_event(self: &Arc<Self>, kind: TopicKind, params: &Value) -> EventStream {
        let workspace = workspace_key(params);
        let (tx, rx) = mpsc::unbounded_channel::<Value>();
        let mut state = self.lock();
        let key = (kind, workspace.clone());
        if let Some(route) = state.routes.get_mut(&key) {
            route.senders.push(tx);
        } else {
            let upstream = self.base.hub.listen(&workspace, kind);
            let task = tokio::spawn(route_frames(Arc::downgrade(self), key.clone(), upstream));
            state.routes.insert(
                key,
                Route {
                    senders: vec![tx],
                    task,
                },
            );
        }
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|value| (Wire::Object(value), rx))
        }))
    }

    /// 帧路由：已拥有 → 投递；否则暂存到同 (kind, workspace, topic) 的在飞订阅。
    fn route(&self, key: &(TopicKind, String), frame: Arc<RoutedFrame>) {
        let mut state = self.lock();
        let owns = state.owned.values().any(|entry| {
            entry.kind == frame.kind
                && entry.subscription_id == frame.subscription_id
                && entry.topic == frame.topic
                && entry.workspace_key == key.1
        });
        if owns {
            if let Some(route) = state.routes.get_mut(key) {
                route
                    .senders
                    .retain(|tx| tx.send(frame.params.clone()).is_ok());
            }
            return;
        }
        let group = format!("{}\0{}\0{}", frame.kind.as_str(), key.1, frame.topic);
        let bytes = serde_json::to_vec(&frame.params).map_or(0, |bytes| bytes.len());
        for pending in state.pending.values_mut() {
            if pending.group_key != group || pending.kind != frame.kind {
                continue;
            }
            if pending.overflow.is_some() || pending.invalidated {
                continue;
            }
            if pending.frames.len() + 1 > MAX_PENDING_FRAMES
                || pending.staged_bytes + bytes > MAX_PENDING_BYTES
            {
                pending.frames.clear();
                pending.staged_bytes = 0;
                pending.overflow = Some("fault.subscription.initialFrameStagingOverflow");
                continue;
            }
            pending.frames.push(Arc::clone(&frame));
            pending.staged_bytes += bytes;
        }
    }
}

async fn route_frames(
    scope: Weak<Scope>,
    key: (TopicKind, String),
    mut upstream: mpsc::UnboundedReceiver<Arc<RoutedFrame>>,
) {
    while let Some(frame) = upstream.recv().await {
        let Some(scope) = scope.upgrade() else {
            return;
        };
        scope.route(&key, frame);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::StreamExt;
    use serde_json::{Value, json};

    use super::super::base::Base;
    use super::super::frames::{RoutedFrame, TopicKind};
    use super::super::scope::{Owned, Scope};
    use crate::session::ClientMode;
    use crate::wire::Wire;

    fn scope() -> Arc<Scope> {
        let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
        let options: escode_host_surface::OptionsFactory = Arc::new(|_| unreachable!("no runtime"));
        let plugin_management =
            crate::domains_plugin::PluginManagement::new(instances.clone(), options.clone(), None);
        let base = Arc::new(Base::new(
            instances,
            options,
            None,
            None,
            plugin_management,
            None,
        ));
        Scope::new(
            base,
            "host-rpc-test".to_owned(),
            ClientMode::DesktopContinuous,
        )
    }

    fn target() -> Value {
        json!({"workspacePath": "/w", "workspaceIdentity": "/w"})
    }

    fn frame(subscription: &str, seq: u64) -> Arc<RoutedFrame> {
        Arc::new(RoutedFrame {
            kind: TopicKind::Conversation,
            topic: "conversation/s1".to_owned(),
            subscription_id: subscription.to_owned(),
            params: json!({"subscriptionId": subscription, "topic": "conversation/s1", "seq": seq}),
        })
    }

    fn owned(subscription: &str) -> Owned {
        Owned {
            kind: TopicKind::Conversation,
            ownership_key: "conversation\0/w\0conversation/s1\0host-rpc-test".to_owned(),
            subscription_id: subscription.to_owned(),
            topic: "conversation/s1".to_owned(),
            workspace_key: "/w".to_owned(),
            target: target(),
        }
    }

    fn key() -> (TopicKind, String) {
        (TopicKind::Conversation, "/w".to_owned())
    }

    async fn seqs(stream: &mut crate::session::EventStream, count: usize) -> Vec<u64> {
        let mut out = Vec::new();
        for _ in 0..count {
            let next = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .expect("frame in time")
                .expect("stream open");
            let Wire::Object(value) = next else {
                panic!("object frame")
            };
            out.push(value["seq"].as_u64().expect("seq"));
        }
        out
    }

    #[tokio::test]
    async fn frames_before_the_ack_are_staged_and_released_in_arrival_order() {
        let scope = scope();
        let mut stream = scope.routed_event(TopicKind::Conversation, &target());
        let pending = scope.begin_pending(TopicKind::Conversation, &target(), "conversation/s1");
        // CLI 通知先于 subscribe 应答到达：同 topic 的帧先暂存（含别人的 subscriptionId）。
        scope.route(&key(), frame("sub-1", 1));
        scope.route(&key(), frame("sub-other", 99));
        scope.route(&key(), frame("sub-1", 2));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), stream.next())
                .await
                .is_err(),
            "nothing is delivered before the ACK"
        );
        scope
            .remember_after_subscribe(pending, owned("sub-1"))
            .expect("ack");
        // ACK 后：只释放 ACK 的 subscriptionId 的暂存帧（保序），随后实时帧接在后面。
        scope.route(&key(), frame("sub-1", 3));
        scope.route(&key(), frame("sub-other", 100));
        assert_eq!(seqs(&mut stream, 3).await, vec![1, 2, 3]);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), stream.next())
                .await
                .is_err(),
            "foreign subscription frames are never delivered"
        );
    }

    #[tokio::test]
    async fn staging_overflow_fails_the_subscription_explicitly() {
        let scope = scope();
        let _stream = scope.routed_event(TopicKind::Conversation, &target());
        let pending = scope.begin_pending(TopicKind::Conversation, &target(), "conversation/s1");
        for seq in 0..1_025 {
            scope.route(&key(), frame("sub-1", seq));
        }
        // 溢出整批清空并显式失败，且需要反向注销（不能留下 runtime 侧订阅）。
        assert_eq!(
            scope.remember_after_subscribe(pending, owned("sub-1")),
            Err(("fault.subscription.initialFrameStagingOverflow", true))
        );
        assert!(!scope.is_owned(&owned("sub-1")));
    }

    #[tokio::test]
    async fn a_late_ack_from_a_restarted_runtime_is_rejected_without_unsubscribing() {
        let scope = scope();
        let pending = scope.begin_pending(TopicKind::Conversation, &target(), "conversation/s1");
        scope.invalidate_runtime("/w", 1);
        // 旧 runtime 的迟到 ACK 可能与新 runtime 复用同一 subId：只丢本地 owner，不反向注销。
        assert_eq!(
            scope.remember_after_subscribe(pending, owned("sub-1")),
            Err(("fault.subscription.runtimeRestarted", false))
        );
        // 换代后登记的 owner 也会被下一次换代清掉。
        let pending = scope.begin_pending(TopicKind::Conversation, &target(), "conversation/s1");
        scope
            .remember_after_subscribe(pending, owned("sub-2"))
            .expect("current generation");
        assert!(scope.is_owned(&owned("sub-2")));
        scope.invalidate_runtime("/w", 2);
        assert!(!scope.is_owned(&owned("sub-2")));
    }
}
