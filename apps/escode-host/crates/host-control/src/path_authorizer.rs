//! 本地媒体路径授权的请求/应答配对（rust-u2-attachments.md §5）。
//!
//! 授权注册表的所有者是 Main；Host 只经控制通道发 `localMediaPreviewAuthorize` 并等待
//! `localMediaPreviewAuthorized`。pending 表是这里唯一的状态：应答按 `requestId` 配对、只消费一次，
//! 未知 `requestId`（stale/重复）直接丢弃。
//!
//! 持有控制队列的写句柄，所以收口时必须 `close()` 释放——否则队列永不关闭、泵不退出
//! （与 `logging::release_sink` 同一约束）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::{ControlMessage, ControlWriter};

const CLOSED: &str = "Control channel closed";

pub struct PathAuthorizer {
    writer: Mutex<Option<ControlWriter>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<String, String>>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl PathAuthorizer {
    pub fn new(writer: ControlWriter) -> Arc<Self> {
        Arc::new(Self {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::default(),
        })
    }

    /// 请求 Main 授权 `path`；返回规范路径，或 Main 给出的原文错误。无超时（TS 同：Main 退出即 EOF 收口）。
    pub async fn authorize(&self, path: &str) -> Result<String, String> {
        let writer = lock(&self.writer)
            .clone()
            .ok_or_else(|| CLOSED.to_owned())?;
        let request_id = format!("media-{}", writer.next_sequence());
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(request_id.clone(), tx);
        let sent = writer
            .control(ControlMessage::LocalMediaPreviewAuthorize {
                request_id: request_id.clone(),
                path: path.to_owned(),
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
        path: Option<String>,
        error: Option<String>,
    ) -> bool {
        let Some(waiter) = lock(&self.pending).remove(request_id) else {
            return false;
        };
        let result = if ok {
            path.ok_or_else(|| "Local media preview path authorization returned no path".to_owned())
        } else {
            Err(error.unwrap_or_else(|| "Local media preview path authorization failed".to_owned()))
        };
        let _ = waiter.send(result);
        true
    }

    /// 收口：释放写句柄，并让所有挂起的请求以 `Control channel closed` 失败。
    pub fn close(&self) {
        lock(&self.writer).take();
        lock(&self.pending).clear();
    }
}

#[cfg(test)]
#[path = "path_authorizer_tests.rs"]
mod tests;
