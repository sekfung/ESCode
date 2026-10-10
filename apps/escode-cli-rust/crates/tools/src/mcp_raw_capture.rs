//! MCP stdio 应答原文旁路（docs/specs/rust-tool-schema-order.md）：rmcp 把 `tools/list` 解析成排序的
//! serde_json Map，声明顺序只存在于原文。这里在 codec 之前旁路读取每一行，暂存含 `result.tools` 的应答原文，
//! 请求完成后登记各工具 inputSchema 的保序文本。
use crate::domain::json_order::Json;
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, ReadBuf};

/// 与 JSON-RPC codec 的单行上限一致。
const MAX_LINE: usize = 8 * 1024 * 1024;
/// 同一连接上并发中的 tools/list 应答极少；超出时清空，查不到只会回落为排序输出。
const MAX_PENDING: usize = 16;

#[derive(Clone, Default)]
pub(super) struct Responses(Arc<Mutex<HashMap<String, String>>>);

impl Responses {
    pub(super) fn offer(&self, line: &[u8]) {
        if !line.windows(7).any(|w| w == b"\"tools\"") {
            return;
        }
        let Some(json) = std::str::from_utf8(line).ok().and_then(|t| Json::parse(t.trim())) else {
            return;
        };
        let (Some(id), Some(_)) = (
            json.get("id"),
            json.get("result").and_then(|r| r.get("tools")).and_then(Json::as_array),
        ) else {
            return;
        };
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if pending.len() >= MAX_PENDING {
            pending.clear();
        }
        pending.insert(id.compact(), String::from_utf8_lossy(line).into_owned());
    }

    /// `tools/list` 完成后调用：取出本请求的原文，登记每个工具 inputSchema 的保序文本。
    pub(super) fn register(&self, id: &rmcp::model::RequestId) {
        let Ok(key) = serde_json::to_string(id) else {
            return;
        };
        let Some(raw) = self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&key) else {
            return;
        };
        let Some(json) = Json::parse(raw.trim()) else {
            return;
        };
        let tools = json.get("result").and_then(|r| r.get("tools")).and_then(Json::as_array);
        for schema in tools.unwrap_or_default().iter().filter_map(|t| t.get("inputSchema")) {
            crate::domain::schema_order::remember(schema);
        }
    }
}

/// 透明旁路：读到的字节原样交给 codec，同时按行交给 `Responses`。
pub(super) struct Tee<R> {
    inner: R,
    line: Vec<u8>,
    overflow: bool,
    responses: Responses,
}

impl<R> Tee<R> {
    pub(super) fn new(inner: R, responses: Responses) -> Self {
        Self { inner, line: Vec::new(), overflow: false, responses }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Tee<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        let this = &mut *self;
        for &byte in &buf.filled()[before..] {
            if byte == b'\n' {
                if !this.overflow {
                    this.responses.offer(&this.line);
                }
                this.line.clear();
                this.overflow = false;
            } else if !this.overflow {
                this.line.push(byte);
                if this.line.len() > MAX_LINE {
                    this.overflow = true;
                    this.line = Vec::new();
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}
