//! Host 内部事实 → Main 的 `HostEvent` 出口（资源采样、agent 计数、CUA 状态等）。
//!
//! 控制通道写句柄在组合根装配；host-rpc 的域只经这个 sink 发布，不直接持有 writer，
//! 也不新增第二条 Main 通信路径。

use std::sync::{Arc, OnceLock};

use serde_json::Value;

pub type HostEventSink = Arc<dyn Fn(Value) + Send + Sync>;

static SINK: OnceLock<HostEventSink> = OnceLock::new();

pub fn set_sink(sink: HostEventSink) {
    let _ = SINK.set(sink);
}

pub fn emit(event: Value) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}
