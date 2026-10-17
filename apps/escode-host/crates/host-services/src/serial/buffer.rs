//! 串口收发记录（S3）：`rust-u2-serial.md` §10。
//!
//! seq 由**连接**统一分配（跨会话、跨关闭重开单调递增），因此同一缓冲内的 seq 可以不连续；
//! 按字节上限淘汰最早的 chunk，并记录 `evicted_through`——游标早于它说明中间有数据缺失。
//!
//! RX 合并（`coalesceWindowMs`）**不在本模块里做定时**：缓冲只收已成段的数据，debounce 由会话层
//! 用「尾部静默窗口」实现（见 `session.rs`），这样缓冲本身是纯数据结构、可确定性测试。

use std::collections::VecDeque;

use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct Chunk {
    pub seq: u64,
    pub at: u64,
    pub direction: &'static str,
    pub source: String,
    pub session_id: Option<String>,
    pub bytes: Vec<u8>,
}

impl Chunk {
    /// 事件载荷（`onData` 的 `SerialPathChunk`：`{...chunk, path}`）。
    pub fn to_json(&self, path: &str) -> Value {
        let mut value = json!({
            "seq": self.seq,
            "at": self.at,
            "direction": self.direction,
            "source": self.source,
            "bytes": self.bytes,
            "path": path,
        });
        if let Some(session_id) = &self.session_id {
            value["sessionId"] = Value::from(session_id.clone());
        }
        value
    }
}

pub struct ChunkBuffer {
    chunks: VecDeque<Chunk>,
    buffered_bytes: usize,
    limit_bytes: usize,
    seq: u64,
    evicted_through: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

impl ChunkBuffer {
    pub fn new(limit_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            buffered_bytes: 0,
            limit_bytes,
            seq: 0,
            evicted_through: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.seq
    }

    /// 入缓冲并按字节上限淘汰（至少保留最新一条）。
    /// `allocate_seq` 由调用方提供——seq 属于连接，不属于缓冲。
    fn push(
        &mut self,
        direction: &'static str,
        source: &str,
        bytes: Vec<u8>,
        at: u64,
        session_id: Option<String>,
        allocate_seq: &mut dyn FnMut() -> u64,
    ) -> Chunk {
        self.seq = allocate_seq();
        let chunk = Chunk {
            seq: self.seq,
            at,
            direction,
            source: source.to_owned(),
            session_id,
            bytes,
        };
        self.buffered_bytes += chunk.bytes.len();
        if direction == "rx" {
            self.rx_bytes += chunk.bytes.len() as u64;
        } else {
            self.tx_bytes += chunk.bytes.len() as u64;
        }
        self.chunks.push_back(chunk.clone());
        while self.buffered_bytes > self.limit_bytes && self.chunks.len() > 1 {
            if let Some(evicted) = self.chunks.pop_front() {
                self.buffered_bytes -= evicted.bytes.len();
                self.evicted_through = evicted.seq;
            }
        }
        chunk
    }

    /// RX 成段：`source` 固定为 `"user"`（外部设备）。
    pub fn push_rx(
        &mut self,
        bytes: Vec<u8>,
        at: u64,
        allocate_seq: &mut dyn FnMut() -> u64,
    ) -> Chunk {
        self.push("rx", "user", bytes, at, None, allocate_seq)
    }

    /// TX 成段：`sessionId` 只在 `source === "agent"` 时带。
    pub fn push_tx(
        &mut self,
        source: &str,
        bytes: Vec<u8>,
        at: u64,
        session_id: Option<String>,
        allocate_seq: &mut dyn FnMut() -> u64,
    ) -> Chunk {
        let session_id = if source == "agent" { session_id } else { None };
        self.push("tx", source, bytes, at, session_id, allocate_seq)
    }

    /// TS `clear`：清空 chunk 与计数，`evictedThrough` 记到当前 seq（之后读到旧游标会标 evicted）。
    pub fn clear(&mut self) {
        self.evicted_through = self.seq;
        self.chunks.clear();
        self.buffered_bytes = 0;
        self.rx_bytes = 0;
        self.tx_bytes = 0;
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "chunks": self.chunks.iter().map(chunk_json).collect::<Vec<_>>(),
            "seq": self.seq,
            "stats": {"rxBytes": self.rx_bytes, "txBytes": self.tx_bytes},
        })
    }
}

fn chunk_json(chunk: &Chunk) -> Value {
    let mut value = json!({
        "seq": chunk.seq,
        "at": chunk.at,
        "direction": chunk.direction,
        "source": chunk.source,
        "bytes": chunk.bytes,
    });
    if let Some(session_id) = &chunk.session_id {
        value["sessionId"] = Value::from(session_id.clone());
    }
    value
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
