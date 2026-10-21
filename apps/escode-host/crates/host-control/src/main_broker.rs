//! Host→Main 专属服务调用（cua-permission / sandbox 注册）。
//!
//! Main 是这些能力的所有者；Host 只经控制通道发 `mainRequest` 并等待 `mainResponse`。
//! pending 表是这里唯一的状态：应答按 `requestId` 配对、只消费一次，未知/重复直接丢弃。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::{ControlMessage, ControlWriter};

const CLOSED: &str = "Control channel closed";

pub struct MainBroker {
    writer: Mutex<Option<ControlWriter>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl MainBroker {
    pub fn new(writer: ControlWriter) -> Arc<Self> {
        Arc::new(Self {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::default(),
        })
    }

    /// 请求 Main 执行 `method`。无超时：Main 退出即控制通道 EOF 收口。
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let writer = lock(&self.writer)
            .clone()
            .ok_or_else(|| CLOSED.to_owned())?;
        let request_id = format!("main-{}", writer.next_sequence());
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(request_id.clone(), tx);
        let sent = writer
            .control(ControlMessage::MainRequest {
                request_id: request_id.clone(),
                method: method.to_owned(),
                params,
            })
            .await;
        if sent.is_err() {
            lock(&self.pending).remove(&request_id);
            return Err(CLOSED.to_owned());
        }
        drop(writer);
        rx.await.unwrap_or_else(|_| Err(CLOSED.to_owned()))
    }

    /// 消费 Main 的应答；未知 `requestId` 返回 false（丢弃）。
    pub fn resolve(
        &self,
        request_id: &str,
        ok: bool,
        result: Option<serde_json::Value>,
        error: Option<String>,
    ) -> bool {
        let Some(waiter) = lock(&self.pending).remove(request_id) else {
            return false;
        };
        let result = if ok {
            Ok(result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(error.unwrap_or_else(|| "Main service request failed".to_owned()))
        };
        let _ = waiter.send(result);
        true
    }

    pub fn close(&self) {
        lock(&self.writer).take();
        lock(&self.pending).clear();
    }
}
