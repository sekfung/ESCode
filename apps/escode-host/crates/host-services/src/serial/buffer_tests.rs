//! 收发缓冲（`rust-u2-serial.md` §10）。
use super::*;

fn seq_allocator() -> impl FnMut() -> u64 {
    let mut next = 0_u64;
    move || {
        next += 1;
        next
    }
}

#[test]
fn rx_and_tx_chunks_get_monotonic_seq_and_stats() {
    let mut buffer = ChunkBuffer::new(1024);
    let mut allocate = seq_allocator();
    buffer.push_rx(b"abc".to_vec(), 1000, &mut allocate);
    buffer.push_tx("user", b"xy".to_vec(), 1001, None, &mut allocate);
    buffer.push_tx(
        "agent",
        b"z".to_vec(),
        1002,
        Some("sess-1".to_owned()),
        &mut allocate,
    );

    let snapshot = buffer.snapshot();
    let chunks = snapshot["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0]["seq"], serde_json::json!(1));
    assert_eq!(chunks[0]["direction"], serde_json::json!("rx"));
    assert_eq!(chunks[0]["source"], serde_json::json!("user"));
    assert_eq!(chunks[1]["seq"], serde_json::json!(2));
    assert_eq!(chunks[2]["seq"], serde_json::json!(3));
    assert_eq!(chunks[2]["sessionId"], serde_json::json!("sess-1"));
    // 非 agent 来源不带 sessionId。
    assert!(chunks[1].get("sessionId").is_none());
    assert_eq!(snapshot["stats"]["rxBytes"], serde_json::json!(3));
    assert_eq!(snapshot["stats"]["txBytes"], serde_json::json!(3));
    assert_eq!(snapshot["seq"], serde_json::json!(3));
    assert_eq!(buffer.last_seq(), 3);
}

/// 超过字节上限时从最旧开始淘汰，且至少保留最新一条。
#[test]
fn eviction_drops_oldest_chunks_and_tracks_the_watermark() {
    let mut buffer = ChunkBuffer::new(5);
    let mut allocate = seq_allocator();
    buffer.push_rx(b"aaa".to_vec(), 1, &mut allocate);
    buffer.push_rx(b"bbb".to_vec(), 2, &mut allocate);
    let snapshot = buffer.snapshot();
    let chunks = snapshot["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 1, "只剩最新一条");
    assert_eq!(chunks[0]["seq"], serde_json::json!(2));

    // 单条就超过上限时也必须保留它（否则 seq 会原地踏步）。
    let mut buffer = ChunkBuffer::new(2);
    let mut allocate = seq_allocator();
    buffer.push_rx(b"12345".to_vec(), 1, &mut allocate);
    assert_eq!(buffer.snapshot()["chunks"].as_array().unwrap().len(), 1);
}

#[test]
fn clear_resets_chunks_and_stats_but_keeps_seq_monotonic() {
    let mut buffer = ChunkBuffer::new(1024);
    let mut allocate = seq_allocator();
    buffer.push_rx(b"abc".to_vec(), 1, &mut allocate);
    buffer.push_tx("user", b"d".to_vec(), 2, None, &mut allocate);
    buffer.clear();
    let snapshot = buffer.snapshot();
    assert_eq!(snapshot["chunks"], serde_json::json!([]));
    assert_eq!(
        snapshot["stats"],
        serde_json::json!({"rxBytes": 0, "txBytes": 0})
    );
    assert_eq!(snapshot["seq"], serde_json::json!(2), "seq 不回退");

    // clear 之后继续写入：seq 继续增长（不回退到 1）。
    buffer.push_rx(b"e".to_vec(), 3, &mut allocate);
    assert_eq!(buffer.last_seq(), 3);
}

/// `onData` 的载荷形状：`{...chunk, path}`。
#[test]
fn event_payload_carries_the_port_path() {
    let chunk = Chunk {
        seq: 7,
        at: 123,
        direction: "rx",
        source: "user".to_owned(),
        session_id: Some("s".to_owned()),
        bytes: b"hi".to_vec(),
    };
    let payload = chunk.to_json("/dev/ttyS0");
    assert_eq!(payload["path"], serde_json::json!("/dev/ttyS0"));
    assert_eq!(payload["seq"], serde_json::json!(7));
    assert_eq!(payload["bytes"], serde_json::json!([104, 105]));
    assert_eq!(payload["sessionId"], serde_json::json!("s"));
}
