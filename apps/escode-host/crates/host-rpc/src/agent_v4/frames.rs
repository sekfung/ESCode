//! Host 级 V4 帧分发（TS `escodeAgentService.ts` 的 `client.onNotification` 中
//! `V4_NOTIFICATIONS.conversationFrame` 分支 + `get{Conversation,SessionsIndex,WorkspaceConfig}FrameEmitter`）。
//!
//! runtime 的三类 topic 帧走同一条通知 `v4/conversation/frame`，按 topic 前缀分流：
//! `sessions-index/` → sessions-index、`workspace-config/` → workspace-config、其余 → conversation。
//! 每个 workspace 实例接一个帧泵（按代次去重），把帧推给该 workspace 的全部监听者。
//!
//! 时序：连接作用域在发 `v4/conversation/subscribe` **之前**调用 [`FrameHub::ensure_pump`]，
//! 所以订阅应答之前到达的帧不会因为泵尚未就位而丢失（暂存由连接作用域负责）。
//!
//! 溢出策略：监听者通道无界——帧泵不能被某条慢连接阻塞（会连带阻塞其他连接的帧与应答）；
//! 连接级积压由出站字节水位 → `v4/connection/flow saturated` → runtime 暂停投递闭环约束。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::mpsc;

/// topic 类别（TS `SubscriptionKind`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TopicKind {
    Conversation,
    SessionsIndex,
    WorkspaceConfig,
}

impl TopicKind {
    pub(crate) fn of(topic: &str) -> Self {
        if topic.starts_with("sessions-index/") {
            Self::SessionsIndex
        } else if topic.starts_with("workspace-config/") {
            Self::WorkspaceConfig
        } else {
            Self::Conversation
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::SessionsIndex => "sessions-index",
            Self::WorkspaceConfig => "workspace-config",
        }
    }
}

/// 一条已分类的帧：`params` 原样（即 TS 的 wire candidate，Renderer 自己解码/拼片）。
#[derive(Clone, Debug)]
pub(crate) struct RoutedFrame {
    pub kind: TopicKind,
    pub topic: String,
    pub subscription_id: String,
    pub params: Value,
}

type Listener = (TopicKind, mpsc::UnboundedSender<Arc<RoutedFrame>>);

/// 帧分发表（Host 内唯一，跨连接共享）。
#[derive(Default)]
pub(crate) struct FrameHub {
    listeners: Mutex<BTreeMap<String, Vec<Listener>>>,
    /// 已接泵的 (workspace 身份键 → 代次)。
    pumps: Mutex<BTreeMap<String, u64>>,
}

impl FrameHub {
    /// 监听一个 workspace 的某类帧（接收端 drop 即注销）。
    pub(crate) fn listen(
        &self,
        workspace_key: &str,
        kind: TopicKind,
    ) -> mpsc::UnboundedReceiver<Arc<RoutedFrame>> {
        let (tx, rx) = mpsc::unbounded_channel();
        lock(&self.listeners)
            .entry(workspace_key.to_owned())
            .or_default()
            .push((kind, tx));
        rx
    }

    /// 确保该实例这一代已接帧泵（幂等）。订阅登记在返回前完成。
    pub(crate) async fn ensure_pump(
        self: &Arc<Self>,
        instance: &Arc<escode_host_runtime::Instance>,
    ) {
        {
            let mut pumps = lock(&self.pumps);
            if pumps.get(&instance.identity) == Some(&instance.generation) {
                return;
            }
            pumps.insert(instance.identity.clone(), instance.generation);
        }
        let mut frames = instance.subscribe().await;
        let hub = Arc::clone(self);
        let identity = instance.identity.clone();
        let generation = instance.generation;
        tokio::spawn(async move {
            while let Some(line) = frames.recv().await {
                if let Some(frame) = parse_frame(&line) {
                    hub.dispatch(&identity, frame);
                }
            }
            if frames.lagged() {
                eprintln!(
                    "escode-host: v4 frame pump for {identity} fell behind and was dropped by the runtime fan-out"
                );
            }
            // 只清自己这一代的泵登记（下一代可能已经接上）。
            let mut pumps = lock(&hub.pumps);
            if pumps.get(&identity) == Some(&generation) {
                pumps.remove(&identity);
            }
        });
    }

    fn dispatch(&self, workspace_key: &str, frame: RoutedFrame) {
        let frame = Arc::new(frame);
        let mut listeners = lock(&self.listeners);
        if let Some(entries) = listeners.get_mut(workspace_key) {
            entries.retain(|(kind, tx)| *kind != frame.kind || tx.send(Arc::clone(&frame)).is_ok());
            if entries.is_empty() {
                listeners.remove(workspace_key);
            }
        }
    }
}

/// 解析一行 runtime 出站帧；只认 `v4/conversation/frame` 且 params 带字符串 `topic`/`subscriptionId`。
/// 其余字段不在 Host 校验（TS 用 zod wire candidate 校验并丢弃非法帧；Rust 只做路由所需的最小校验，
/// 形状校验由 Renderer 的解码器负责——登记差异 rust-u2-agent-v4.md §6）。
pub(crate) fn parse_frame(line: &str) -> Option<RoutedFrame> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value["method"] != "v4/conversation/frame" {
        return None;
    }
    let params = value.get("params")?.clone();
    let topic = params["topic"].as_str()?.to_owned();
    let subscription_id = params["subscriptionId"].as_str()?.to_owned();
    Some(RoutedFrame {
        kind: TopicKind::of(&topic),
        topic,
        subscription_id,
        params,
    })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_are_classified_by_topic_prefix() {
        assert_eq!(
            TopicKind::of("conversation/sess_1"),
            TopicKind::Conversation
        );
        assert_eq!(TopicKind::of("sessions-index//w"), TopicKind::SessionsIndex);
        assert_eq!(
            TopicKind::of("workspace-config//w"),
            TopicKind::WorkspaceConfig
        );
        let line = json!({"method": "v4/conversation/frame", "params": {"topic": "conversation/s", "subscriptionId": "sub_1", "kind": "snapshot"}}).to_string();
        let frame = parse_frame(&line).expect("frame");
        assert_eq!(frame.subscription_id, "sub_1");
        assert_eq!(frame.params["kind"], "snapshot");
        for other in [
            json!({"method": "state.updated", "params": {"topic": "conversation/s", "subscriptionId": "x"}}),
            json!({"method": "v4/conversation/frame", "params": {"topic": "conversation/s"}}),
            json!({"id": "host-1", "result": {}}),
        ] {
            assert!(parse_frame(&other.to_string()).is_none(), "{other}");
        }
    }

    #[test]
    fn dispatch_reaches_only_matching_kind_and_prunes_closed_listeners() {
        let hub = FrameHub::default();
        let mut conversation = hub.listen("/w", TopicKind::Conversation);
        let mut index = hub.listen("/w", TopicKind::SessionsIndex);
        let other = hub.listen("/other", TopicKind::Conversation);
        drop(other);
        let frame = |topic: &str| RoutedFrame {
            kind: TopicKind::of(topic),
            topic: topic.to_owned(),
            subscription_id: "sub".to_owned(),
            params: json!({}),
        };
        hub.dispatch("/w", frame("conversation/s"));
        hub.dispatch("/w", frame("sessions-index//w"));
        hub.dispatch("/other", frame("conversation/s"));
        assert_eq!(
            conversation.try_recv().expect("conversation").topic,
            "conversation/s"
        );
        assert!(conversation.try_recv().is_err());
        assert_eq!(
            index.try_recv().expect("index").kind,
            TopicKind::SessionsIndex
        );
        assert!(
            !lock(&hub.listeners).contains_key("/other"),
            "closed listener pruned"
        );
    }
}
