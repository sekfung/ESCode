//! RPC 线协议分帧（与 `packages/rpc/src/protocol.ts` 的 `ProtocolMessage` / `SocketProtocol` 对等）。
//!
//! 远端 server 的 stdio 在 `escode-hello` 握手后使用该帧面：
//!
//! ```text
//! type(1) id(4) ack(4) length(4) payload(length)
//! ```
//!
//! 所有整数大端。只有 `Regular`（type=1）帧带业务载荷；其它类型（Ack/KeepAlive/Control/
//! Disconnect 等）与 TS `SocketProtocol` 一样被跳过，不在这一层做确认或重连。
//!
//! 与 TS 的差异只有一条：TS 不限制 `length`，这里给出显式上限，避免异常帧触发无界分配。

use std::fmt;

pub const HEADER_SIZE: usize = 13;
pub const TYPE_REGULAR: u8 = 1;

/// 单帧上限：远端附件/历史帧可能很大，但 256 MiB 以上视为协议错误。
pub const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub kind: u8,
    pub id: u32,
    pub ack: u32,
    pub length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    /// `length` 超过 [`MAX_FRAME_BYTES`]。
    FrameTooLarge(u32),
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge(length) => {
                write!(formatter, "rpc frame length {length} exceeds limit")
            }
        }
    }
}

impl std::error::Error for WireError {}

/// 编码一帧；`SocketProtocol.send` 使用 `kind=TYPE_REGULAR, id=0, ack=0`。
pub fn encode_frame(kind: u8, id: u32, ack: u32, payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(payload.len()).expect("payload fits u32");
    let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
    bytes.push(kind);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&ack.to_be_bytes());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

/// `SocketProtocol.send` 的线形：Regular + id/ack 0。
pub fn encode_regular(payload: &[u8]) -> Vec<u8> {
    encode_frame(TYPE_REGULAR, 0, 0, payload)
}

/// 增量解码：分片/粘包都按 `SocketProtocol.readMessages` 的语义处理。
#[derive(Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    offset: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if self.offset > 0 {
            self.buffer.drain(..self.offset);
            self.offset = 0;
        }
        self.buffer.extend_from_slice(chunk);
    }

    /// 已缓存但尚未消费的字节数。
    pub fn buffered(&self) -> usize {
        self.buffer.len() - self.offset
    }

    /// 取下一条 `Regular` 载荷；非 Regular 帧被消费但不返回。
    ///
    /// 返回 `None` 表示当前缓冲不足一帧（调用方继续 `push`），不消费半帧。
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        loop {
            let available = self.buffered();
            if available < HEADER_SIZE {
                return Ok(None);
            }
            let header = self.peek_header();
            let length = header.length as usize;
            if length > MAX_FRAME_BYTES {
                return Err(WireError::FrameTooLarge(header.length));
            }
            if available < HEADER_SIZE + length {
                return Ok(None);
            }
            self.offset += HEADER_SIZE + length;
            if header.kind == TYPE_REGULAR {
                let start = self.offset - length;
                return Ok(Some(self.buffer[start..self.offset].to_vec()));
            }
        }
    }

    fn peek_header(&self) -> FrameHeader {
        let header = &self.buffer[self.offset..self.offset + HEADER_SIZE];
        FrameHeader {
            kind: header[0],
            id: u32::from_be_bytes([header[1], header[2], header[3], header[4]]),
            ack: u32::from_be_bytes([header[5], header[6], header[7], header[8]]),
            length: u32::from_be_bytes([header[9], header[10], header[11], header[12]]),
        }
    }
}

#[cfg(test)]
#[path = "rpc_wire_tests.rs"]
mod tests;
