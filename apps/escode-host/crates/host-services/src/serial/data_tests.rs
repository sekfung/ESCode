//! 数据面（`rust-u2-serial.md` §10）：seq 分配、RX 合并、TX 记账。
use super::*;
use std::sync::atomic::AtomicU64;

use std::sync::Mutex as StdMutex;

fn sink() -> (
    Arc<DataSink>,
    Arc<AtomicU64>,
    Arc<StdMutex<Vec<serde_json::Value>>>,
) {
    let seq = Arc::new(AtomicU64::new(0));
    let events = Arc::new(StdMutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let data = DataSink::new(
        Arc::clone(&seq),
        Arc::new(move |chunk| {
            sink_events
                .lock()
                .unwrap()
                .push(chunk.to_json("/dev/ttyS0"))
        }),
    );
    (data, seq, events)
}

#[test]
fn tx_is_recorded_before_the_actual_write_with_a_shared_seq() {
    let (data, seq, events) = sink();
    let first = data.push_tx("user", b"ping".to_vec(), None);
    let second = data.push_tx("agent", b"pong".to_vec(), Some("sess-1".to_owned()));
    assert_eq!((first, second), (1, 2));
    assert_eq!(seq.load(std::sync::atomic::Ordering::SeqCst), 2);
    let snapshot = data.snapshot();
    assert_eq!(snapshot["stats"]["txBytes"], serde_json::json!(8));
    let events = events.lock().unwrap();
    assert_eq!(events[0]["direction"], serde_json::json!("tx"));
    assert_eq!(events[1]["sessionId"], serde_json::json!("sess-1"));
}

/// RX 走尾部静默窗口：多次 receive 在窗口内合并成一段。
#[tokio::test]
async fn rx_coalesces_within_the_quiet_window() {
    let (data, _seq, _events) = sink();
    data.receive(b"ab".to_vec());
    data.receive(b"cd".to_vec());
    // 窗口内还没有成段（10ms 未到）。
    assert_eq!(data.snapshot()["chunks"], serde_json::json!([]));
    tokio::time::sleep(Duration::from_millis(40)).await;
    let snapshot = data.snapshot();
    let chunks = snapshot["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 1, "两次 receive 合并成一段: {snapshot}");
    assert_eq!(chunks[0]["bytes"], serde_json::json!([97, 98, 99, 100]));
    assert_eq!(chunks[0]["direction"], serde_json::json!("rx"));
}

/// 超过合并上限立即成段，不等窗口。
#[tokio::test]
async fn rx_over_the_coalesce_limit_flushes_immediately() {
    let (data, _seq, _events) = sink();
    data.receive(vec![1_u8; COALESCE_MAX_BYTES + 1]);
    let snapshot = data.snapshot();
    assert_eq!(
        snapshot["chunks"].as_array().unwrap().len(),
        1,
        "超限立即成段: {snapshot}"
    );
}

/// TX 前先收口 pending RX：回显必须出现在设备响应之前。
#[tokio::test]
async fn tx_flushes_pending_rx_first() {
    let (data, _seq, events) = sink();
    data.receive(b"rx".to_vec());
    let seq = data.push_tx("user", b"tx".to_vec(), None);
    assert_eq!(seq, 2, "先 flush 的 RX 拿到 seq 1");
    let events = events.lock().unwrap();
    assert_eq!(events[0]["direction"], serde_json::json!("rx"));
    assert_eq!(events[1]["direction"], serde_json::json!("tx"));
    assert_eq!(events[1]["seq"], serde_json::json!(2));
}

#[test]
fn clear_resets_counters_and_shutdown_flushes() {
    let (data, _seq, _events) = sink();
    data.push_tx("user", b"abc".to_vec(), None);
    data.clear();
    assert_eq!(
        data.snapshot()["stats"],
        serde_json::json!({"rxBytes": 0, "txBytes": 0})
    );
    assert_eq!(data.snapshot()["chunks"], serde_json::json!([]));
    assert_eq!(data.last_seq(), 1, "seq 不回退");
}
