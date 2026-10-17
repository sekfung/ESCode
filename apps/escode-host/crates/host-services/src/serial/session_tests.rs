//! 会话状态机与重连（`rust-u2-serial.md` §9）。
use super::*;
use crate::serial::transport::testing::FakeTransport;
use std::sync::Arc;
use std::sync::Mutex;

fn make_session() -> (
    Arc<FakeTransport>,
    SerialSession,
    Arc<Mutex<Vec<serde_json::Value>>>,
) {
    let transport = Arc::new(FakeTransport::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let session = SerialSession::new(
        "/dev/ttyS0".to_owned(),
        transport.clone(),
        Arc::new(move |status| sink.lock().unwrap().push(status)),
    );
    (transport, session, events)
}

fn config(auto_reconnect: bool) -> SerialConfig {
    SerialConfig {
        baud_rate: 115200,
        data_bits: 8,
        parity: super::super::transport::Parity::None,
        stop_bits: super::super::transport::StopBits::One,
        rtscts: false,
        auto_reconnect,
    }
}

#[tokio::test]
async fn open_close_transitions_and_status_events() {
    let (transport, first, events) = make_session();
    assert_eq!(first.state(), "closed", "新建会话是 closed");

    first.open(config(false)).await.unwrap();
    assert_eq!(first.state(), "open");
    let status = first.current_status();
    assert_eq!(status["state"], serde_json::json!("open"));
    assert_eq!(status["path"], serde_json::json!("/dev/ttyS0"));
    assert_eq!(status["config"]["baudRate"], serde_json::json!(115200));
    assert_eq!(
        status["signals"],
        serde_json::json!({"dtr": true, "rts": true})
    );

    // 状态事件按 opening → open 顺序落过。
    let states: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .map(|event| event["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(states, vec!["opening", "open"], "TS 不在构造时发状态");

    first.close().await;
    assert_eq!(first.state(), "closed");
    // 幂等：再关一次不报错、不重复关端口。
    first.close().await;
    assert_eq!(
        transport.closed.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn open_failure_lands_in_error_state() {
    let (transport, session, _events) = make_session();
    *transport.fail.lock().unwrap() =
        Some(SerialOpenError::new("busy", "Access denied".to_owned()));
    let error = session.open(config(false)).await.unwrap_err();
    assert_eq!(error.code, "busy");
    let status = session.current_status();
    assert_eq!(status["state"], serde_json::json!("error"));
    assert_eq!(status["error"]["code"], serde_json::json!("busy"));
}

/// 拔掉设备：autoReconnect 为真进 `disconnected`；设备回来后 `reconnect` 能重新打开。
#[tokio::test]
async fn disconnect_then_reconnect_keeps_auto_reconnect_semantics() {
    let (transport, session, _events) = make_session();
    session.open(config(true)).await.unwrap();
    session.handle_disconnect().await;
    assert_eq!(session.state(), "disconnected", "autoReconnect 时不是终态");

    session.reconnect().await.unwrap();
    assert_eq!(session.state(), "open");
    assert_eq!(
        transport.opened.lock().unwrap().len(),
        2,
        "第二次打开是重连"
    );

    // autoReconnect 为假：断开直接回 closed。
    let (other_transport, other, _events) = make_session();
    other.open(config(false)).await.unwrap();
    other.handle_disconnect().await;
    assert_eq!(other.state(), "closed");
    // closed 状态下 reconnect 是空操作（不会偷偷打开）。
    other.reconnect().await.unwrap();
    assert_eq!(other.state(), "closed");
    assert_eq!(other_transport.opened.lock().unwrap().len(), 1);
}
