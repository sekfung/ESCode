use super::{FrameDecoder, HEADER_SIZE, WireError, encode_frame, encode_regular};

/// 与 TS `writeProtocolMessage` 逐字节一致（大端 13 字节头）。
#[test]
fn encodes_the_typescript_header_layout() {
    let bytes = encode_frame(1, 0x0102_0304, 0x0506_0708, b"hi");
    assert_eq!(
        bytes,
        vec![
            0x01, // type
            0x01, 0x02, 0x03, 0x04, // id
            0x05, 0x06, 0x07, 0x08, // ack
            0x00, 0x00, 0x00, 0x02, // length
            b'h', b'i',
        ]
    );
    assert_eq!(
        encode_regular(b""),
        vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

/// 一次 push 里多帧按序返回；非 Regular 帧被跳过。
#[test]
fn decodes_multiple_frames_and_skips_non_regular() {
    let mut bytes = Vec::new();
    bytes.extend(encode_regular(b"a"));
    bytes.extend(encode_frame(9, 1, 2, b"keepalive"));
    bytes.extend(encode_regular(b""));
    bytes.extend(encode_frame(3, 1, 2, b"ack"));
    bytes.extend(encode_regular(b"bc"));
    let mut decoder = FrameDecoder::new();
    decoder.push(&bytes);
    assert_eq!(decoder.next_frame().unwrap(), Some(b"a".to_vec()));
    assert_eq!(decoder.next_frame().unwrap(), Some(Vec::new()));
    assert_eq!(decoder.next_frame().unwrap(), Some(b"bc".to_vec()));
    assert_eq!(decoder.next_frame().unwrap(), None);
    assert_eq!(decoder.buffered(), 0);
}

/// header 与 payload 分片到达：不足一帧时返回 None，补齐后不丢字节。
#[test]
fn waits_for_split_header_and_payload() {
    let frame = encode_regular(b"hello");
    let mut decoder = FrameDecoder::new();
    for (index, byte) in frame.iter().enumerate() {
        decoder.push(std::slice::from_ref(byte));
        if index < frame.len() - 1 {
            assert_eq!(decoder.next_frame().unwrap(), None, "byte {index}");
        }
    }
    assert_eq!(decoder.next_frame().unwrap(), Some(b"hello".to_vec()));
}

/// 半帧不会提前消费；后续补齐仍能正确解码。
#[test]
fn does_not_consume_incomplete_frames() {
    let frame = encode_regular(b"abcdef");
    let (head, tail) = frame.split_at(HEADER_SIZE + 2);
    let mut decoder = FrameDecoder::new();
    decoder.push(head);
    assert_eq!(decoder.next_frame().unwrap(), None);
    assert_eq!(decoder.buffered(), head.len());
    decoder.push(tail);
    assert_eq!(decoder.next_frame().unwrap(), Some(b"abcdef".to_vec()));
}

/// 超过上限的 length 显式报错，不做无界分配。
#[test]
fn rejects_oversized_frames() {
    let mut header = vec![1u8];
    header.extend_from_slice(&0u32.to_be_bytes());
    header.extend_from_slice(&0u32.to_be_bytes());
    header.extend_from_slice(&u32::MAX.to_be_bytes());
    let mut decoder = FrameDecoder::new();
    decoder.push(&header);
    assert_eq!(
        decoder.next_frame(),
        Err(WireError::FrameTooLarge(u32::MAX))
    );
}
