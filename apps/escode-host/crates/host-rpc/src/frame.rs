//! `@escode/rpc` 的 13 字节帧（packages/rpc/src/protocol.ts `SocketProtocol`）。
//!
//! `[type: u8] [id: u32BE] [ack: u32BE] [length: u32BE]` + `length` 字节 body。
//! 帧在**字节流**上切分：一个 WebSocket 消息可能包含半帧或多帧（TS ChunkStream 同语义）。

/// TS `ProtocolMessageType::Regular`。
pub const TYPE_REGULAR: u8 = 1;
pub const HEADER_SIZE: usize = 13;

/// 单帧上限：畸形长度不得让服务端无限缓冲。
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 拆帧器：累积字节、按序吐出完整 body。
#[derive(Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一段字节（一个 WS 消息）。
    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// 取下一个完整帧的 body；不足一帧时返回 `None`（等待后续字节）。
    /// 非 `Regular` 帧（Control/Ack/Disconnect）在协议面暂不使用，跳过。
    pub fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        loop {
            if self.buffer.len() < HEADER_SIZE {
                return Ok(None);
            }
            let kind = self.buffer[0];
            let length = u32::from_be_bytes([
                self.buffer[9],
                self.buffer[10],
                self.buffer[11],
                self.buffer[12],
            ]) as usize;
            anyhow::ensure!(length <= MAX_FRAME_BYTES, "RPC frame exceeds size limit");
            if self.buffer.len() < HEADER_SIZE + length {
                // body 未到齐时不能先消费 header（TS 同注释：否则拼上后续字节后长度信息就丢了）。
                return Ok(None);
            }
            self.buffer.drain(..HEADER_SIZE);
            let body: Vec<u8> = self.buffer.drain(..length).collect();
            if kind == TYPE_REGULAR {
                return Ok(Some(body));
            }
        }
    }
}

/// 把一条 body 编码成一个 Regular 帧（id/ack 为 0；客户端按顺序消费）。
pub fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + body.len());
    out.push(TYPE_REGULAR);
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_split_across_messages() {
        let mut reader = FrameReader::new();
        let one = frame(b"hello");
        let two = frame(b"world");
        // 先灌入「一帧半」，只应吐出第一帧。
        reader.push(&one);
        reader.push(&two[..5]);
        assert_eq!(
            reader.next_frame().unwrap().as_deref(),
            Some(b"hello".as_slice())
        );
        assert!(reader.next_frame().unwrap().is_none(), "第二帧还差 body");
        reader.push(&two[5..]);
        assert_eq!(
            reader.next_frame().unwrap().as_deref(),
            Some(b"world".as_slice())
        );
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn multiple_frames_in_one_message() {
        let mut reader = FrameReader::new();
        let mut batch = frame(b"a");
        batch.extend(frame(b"bb"));
        reader.push(&batch);
        assert_eq!(
            reader.next_frame().unwrap().as_deref(),
            Some(b"a".as_slice())
        );
        assert_eq!(
            reader.next_frame().unwrap().as_deref(),
            Some(b"bb".as_slice())
        );
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn oversized_length_is_rejected() {
        let mut reader = FrameReader::new();
        let mut header = vec![TYPE_REGULAR];
        header.extend_from_slice(&0_u32.to_be_bytes());
        header.extend_from_slice(&0_u32.to_be_bytes());
        header.extend_from_slice(&(MAX_FRAME_BYTES as u32 + 1).to_be_bytes());
        reader.push(&header);
        assert!(reader.next_frame().is_err());
    }

    #[test]
    fn non_regular_frames_are_skipped() {
        let mut reader = FrameReader::new();
        let mut control = frame(b"control");
        control[0] = 2; // Control
        reader.push(&control);
        reader.push(&frame(b"regular"));
        assert_eq!(
            reader.next_frame().unwrap().as_deref(),
            Some(b"regular".as_slice())
        );
    }
}
