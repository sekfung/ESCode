//! `window-controller` 的连接作用域：订阅号登记 + 帧投递。
//!
//! TS 的 controller 是「窗口 Host 内、每 attachment 一个 emitter + 订阅表」；Rust 按 **RPC 连接**
//! 作用域（登记差异 §5-2）。首帧在 `subscribeControllerV4` 内即时发出，而 `EventListen` 是另一条
//! 请求（连接层并发处理）：监听者未 attach 前的帧进有界早期缓冲，attach 时按到达序补发。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use serde_json::Value;
use tokio::sync::mpsc;

use super::projection::Projection;
use crate::session::{EventStream, RpcError};
use crate::wire::Wire;

/// 早期缓冲上限：一次订阅至多一帧快照，正常远低于该值；溢出丢最旧。
const MAX_EARLY_FRAMES: usize = 256;

#[derive(Default)]
struct State {
    subscriptions: BTreeMap<String, ()>,
    listeners: Vec<mpsc::UnboundedSender<Value>>,
    early: VecDeque<Value>,
    disposed: bool,
}

pub(crate) struct Scope {
    projector: Arc<Projection>,
    state: Mutex<State>,
}

impl Scope {
    pub(crate) fn new(projector: Arc<Projection>) -> Arc<Self> {
        Arc::new(Self {
            projector,
            state: Mutex::new(State::default()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 帧出口：有监听者即投递；否则进早期缓冲（溢出丢最旧）。
    fn sink(self: &Arc<Self>) -> super::projection::FrameSink {
        let weak = Arc::downgrade(self);
        Arc::new(move |frame: Value| {
            let Some(scope) = Weak::upgrade(&weak) else {
                return;
            };
            let mut state = scope.lock();
            if state.disposed {
                return;
            }
            if state.listeners.is_empty() {
                if state.early.len() >= MAX_EARLY_FRAMES {
                    state.early.pop_front();
                }
                state.early.push_back(frame);
                return;
            }
            state.listeners.retain(|tx| tx.send(frame.clone()).is_ok());
        })
    }

    /// `subscribeControllerV4`：投影侧登记订阅号并按 TS 顺序先发快照帧、再回 ack。
    pub(crate) fn subscribe(self: &Arc<Self>, topic: &str) -> Result<Value, RpcError> {
        let ack = self.projector.subscribe(topic, self.sink())?;
        let subscription_id = ack["ack"]["subscriptionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        // 登记与收口在同一把锁下判定先后：连接已收口则立刻撤销刚登记的投影订阅，
        // 否则它既不在 `subscriptions`（`dispose` 看不到）又留在投影里，永远泄漏。
        let disposed = {
            let mut state = self.lock();
            if state.disposed {
                true
            } else {
                state.subscriptions.insert(subscription_id.clone(), ());
                false
            }
        };
        if disposed {
            self.projector.unsubscribe(&subscription_id);
            return Err(RpcError::new("Error", "Controller 连接已关闭"));
        }
        Ok(ack)
    }

    pub(crate) fn resync(&self, subscription_id: &str) -> Result<Value, RpcError> {
        self.projector.resync(subscription_id)
    }

    pub(crate) fn unsubscribe(&self, subscription_id: &str) {
        self.projector.unsubscribe(subscription_id);
        self.lock().subscriptions.remove(subscription_id);
    }

    /// `onDynamicControllerFrame()`：新监听者接管早期缓冲。
    pub(crate) fn listen(self: &Arc<Self>) -> EventStream {
        let (tx, rx) = mpsc::unbounded_channel::<Value>();
        {
            let mut state = self.lock();
            state.listeners.push(tx);
            for frame in std::mem::take(&mut state.early) {
                state.listeners.retain(|tx| tx.send(frame.clone()).is_ok());
            }
        }
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|value| (Wire::Object(value), rx))
        }))
    }

    /// 连接收口（TS `createAttachmentService().dispose()`）：注销该连接登记的全部订阅号。
    pub(crate) fn dispose(&self) {
        let ids: Vec<String> = {
            let mut state = self.lock();
            state.disposed = true;
            state.listeners.clear();
            state.early.clear();
            std::mem::take(&mut state.subscriptions)
                .into_keys()
                .collect()
        };
        self.projector.unsubscribe_all(&ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> Arc<Projection> {
        let counter = std::sync::atomic::AtomicU64::new(0);
        Arc::new(Projection::new(Box::new(move || {
            format!(
                "id-{}",
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            )
        })))
    }

    #[test]
    fn subscribe_after_dispose_is_rejected_and_leaves_no_projection_subscriber() {
        let projector = projection();
        let scope = Scope::new(Arc::clone(&projector));
        scope
            .subscribe("controller/tasks-index")
            .expect("subscribe");
        assert_eq!(projector.subscriber_count(), 1);
        scope.dispose();
        assert_eq!(projector.subscriber_count(), 0, "收口注销已登记的订阅");
        let error = scope
            .subscribe("controller/tasks-index")
            .expect_err("closed");
        assert_eq!(error.message, "Controller 连接已关闭");
        assert_eq!(
            projector.subscriber_count(),
            0,
            "收口后的订阅不得泄漏在投影里"
        );
    }

    #[test]
    fn early_frames_are_replayed_to_the_first_listener() {
        let projector = projection();
        let scope = Scope::new(projector);
        scope.subscribe("controller/workspaces").expect("subscribe");
        let mut stream = scope.listen();
        let first =
            futures_util::FutureExt::now_or_never(futures_util::StreamExt::next(&mut stream));
        assert!(
            matches!(first, Some(Some(Wire::Object(_)))),
            "首帧在监听者接入前产生，接入后补发"
        );
    }
}
