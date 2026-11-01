//! 连接受众广播：一条低频事件源（workspace 列表变更、未读提示）分发给任意数量的监听流。
//!
//! 与 `WorkspaceEvents` 的按身份键订阅互补：这里的监听者不预先知道身份键，消费方自行按事件字段判断。
//! 发送失败的监听者在下一次 `emit` 时被移除（流 drop 即关闭通道），不需要显式的取消路径。

use std::sync::{Mutex, MutexGuard};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::session::EventStream;
use crate::wire::Wire;

#[derive(Default)]
pub(crate) struct EventFanout {
    listeners: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
}

impl EventFanout {
    pub(crate) fn listen(&self) -> EventStream {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().push(tx);
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|value| (Wire::Object(value), rx))
        }))
    }

    pub(crate) fn emit(&self, value: &Value) {
        let mut listeners = self.lock();
        if listeners.is_empty() {
            return;
        }
        listeners.retain(|tx| tx.send(value.clone()).is_ok());
    }

    fn lock(&self) -> MutexGuard<'_, Vec<mpsc::UnboundedSender<Value>>> {
        self.listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use serde_json::json;

    #[tokio::test]
    async fn every_listener_receives_every_event() {
        let fanout = EventFanout::default();
        let mut first = fanout.listen();
        let mut second = fanout.listen();
        fanout.emit(&json!({"a": 1}));
        fanout.emit(&json!({"a": 2}));
        for stream in [&mut first, &mut second] {
            for expected in [1, 2] {
                match stream.next().await.expect("event") {
                    Wire::Object(value) => assert_eq!(value["a"], expected),
                    other => panic!("unexpected wire: {other:?}"),
                }
            }
        }
    }

    #[tokio::test]
    async fn dropped_listeners_are_pruned_on_the_next_emit() {
        let fanout = EventFanout::default();
        let mut kept = fanout.listen();
        drop(fanout.listen());
        fanout.emit(&json!({"a": 1}));
        assert_eq!(fanout.lock().len(), 1);
        assert!(kept.next().await.is_some());
    }
}
