//! `serial` 服务级用例（S2 会话 + S3 数据面）。
use super::tests::fake_enum::FakeEnumerator;
use super::*;
use crate::serial::enumerate::{port, port_with};
use crate::serial::transport::testing::FakeTransport;

// ── S2：会话 ──────────────────────────────────────────────────────────────

fn open_config(auto_reconnect: bool) -> serde_json::Value {
    serde_json::json!({
        "baudRate": 115200, "dataBits": 8, "parity": "none",
        "stopBits": 1, "rtscts": false, "autoReconnect": auto_reconnect
    })
}

fn s2_service(fake: &Arc<FakeEnumerator>) -> SerialService {
    SerialService::new(fake.clone(), Arc::new(FakeTransport::default()))
        .with_poll_interval(Duration::from_millis(10))
}

#[tokio::test]
async fn open_and_close_drive_snapshot_and_session_list() {
    let fake = FakeEnumerator::new(vec![port_with("/dev/ttyS0", &[])]);
    let service = s2_service(&fake);
    service
        .open("/dev/ttyS0", &open_config(false))
        .await
        .unwrap();

    let snapshot = service.get_snapshot("/dev/ttyS0");
    assert_eq!(snapshot["status"]["state"], serde_json::json!("open"));
    assert_eq!(
        snapshot["status"]["config"]["baudRate"],
        serde_json::json!(115200)
    );
    assert_eq!(snapshot["chunks"], serde_json::json!([]));
    let sessions = service.list_sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["path"], serde_json::json!("/dev/ttyS0"));

    service.close("/dev/ttyS0").await;
    assert_eq!(
        service.get_snapshot("/dev/ttyS0")["status"]["state"],
        serde_json::json!("closed")
    );
    assert!(service.list_sessions().is_empty(), "关闭后不再是活动会话");

    // 未知路径回 closed 空快照。
    let unknown = service.get_snapshot("/dev/nope");
    assert_eq!(unknown["status"]["state"], serde_json::json!("closed"));
    assert_eq!(unknown["status"]["path"], serde_json::json!("/dev/nope"));
}

#[tokio::test]
async fn the_fifth_active_session_is_rejected() {
    let fake = FakeEnumerator::new(
        (0..6)
            .map(|index| port(&format!("/dev/s{index}")))
            .collect(),
    );
    let service = s2_service(&fake);
    for index in 0..4 {
        service
            .open(&format!("/dev/s{index}"), &open_config(false))
            .await
            .unwrap();
    }
    let error = service
        .open("/dev/s4", &open_config(false))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalidInput");
    assert_eq!(
        error.message,
        "At most 4 serial ports can be open at the same time"
    );
    // 已经打开的路径再开一次不算新增（不触发上限）。
    service.open("/dev/s0", &open_config(false)).await.unwrap();
}

/// 轮询发现设备消失 → `disconnected`；设备回来 → 自动重连回 `open`。
#[tokio::test]
async fn poll_detects_disconnect_and_reconnects_when_the_device_returns() {
    let fake = FakeEnumerator::new(vec![port("/dev/ttyS0")]);
    let service = s2_service(&fake);
    service
        .open("/dev/ttyS0", &open_config(true))
        .await
        .unwrap();
    assert_eq!(
        service.get_snapshot("/dev/ttyS0")["status"]["state"],
        serde_json::json!("open")
    );

    // 设备消失：轮询把它判成 disconnected（autoReconnect=true）。
    fake.set_ports(vec![]);
    service.set_watching(true).await; // 触发轮询（S1 的 watching 也会驱动 poll）
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while service.get_snapshot("/dev/ttyS0")["status"]["state"] != serde_json::json!("disconnected")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "应在一秒内判成 disconnected"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 设备回来：轮询触发 reconnect，状态回到 open。
    fake.set_ports(vec![port("/dev/ttyS0")]);
    while service.get_snapshot("/dev/ttyS0")["status"]["state"] != serde_json::json!("open") {
        assert!(std::time::Instant::now() < deadline, "设备回来应自动重连");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    service.shutdown();
}

/// autoReconnect=false 时设备消失直接回 closed，且不会自己重开。
#[tokio::test]
async fn disconnect_without_auto_reconnect_closes_the_session() {
    let fake = FakeEnumerator::new(vec![port("/dev/ttyS0")]);
    let service = s2_service(&fake);
    service
        .open("/dev/ttyS0", &open_config(false))
        .await
        .unwrap();
    fake.set_ports(vec![]);
    service.set_watching(true).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while service.get_snapshot("/dev/ttyS0")["status"]["state"] != serde_json::json!("closed") {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 设备回来也不重开。
    fake.set_ports(vec![port("/dev/ttyS0")]);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        service.get_snapshot("/dev/ttyS0")["status"]["state"],
        serde_json::json!("closed")
    );
    service.shutdown();
}

/// 状态事件按 `opening → open` 落到 `onStatus`。
#[tokio::test]
async fn status_events_are_broadcast() {
    let fake = FakeEnumerator::new(vec![port("/dev/ttyS0")]);
    let service = s2_service(&fake);
    let mut events = service.subscribe_status();
    service
        .open("/dev/ttyS0", &open_config(false))
        .await
        .unwrap();
    let first = events.recv().await.expect("至少一条状态事件");
    assert_eq!(first["path"], serde_json::json!("/dev/ttyS0"));
    assert_eq!(first["state"], serde_json::json!("opening"));
    let second = events.recv().await.expect("open 状态");
    assert_eq!(second["state"], serde_json::json!("open"));
    assert_eq!(
        second["signals"],
        serde_json::json!({"dtr": true, "rts": true})
    );
    service.shutdown();
}

// ── S3：数据面 ────────────────────────────────────────────────────────────

fn data_service(fake: &Arc<FakeEnumerator>) -> (SerialService, Arc<FakeTransport>) {
    let transport = Arc::new(FakeTransport::default());
    let service = SerialService::new(fake.clone(), transport.clone())
        .with_poll_interval(Duration::from_millis(10));
    (service, transport)
}

/// 写入：先记 TX 再真正写，回包给本次写入的 seq；设备字节经 onData 上送。
#[tokio::test]
async fn write_records_tx_and_device_bytes_arrive_through_on_data() {
    let fake = FakeEnumerator::new(vec![port("/dev/ttyS0")]);
    let (service, transport) = data_service(&fake);
    let mut data_events = service.subscribe_data();
    service
        .open("/dev/ttyS0", &open_config(false))
        .await
        .unwrap();

    let seq = service
        .write("/dev/ttyS0", b"ping".to_vec(), "user", None)
        .await
        .unwrap();
    assert_eq!(seq, 1, "第一次写入拿到 seq 1");
    assert_eq!(
        transport.writes.lock().unwrap().as_slice(),
        &[b"ping".to_vec()]
    );

    // TX 也走 onData（先到的那条是它）。
    let tx_event = tokio::time::timeout(Duration::from_secs(2), data_events.recv())
        .await
        .expect("应收到 TX 事件")
        .expect("流未关闭");
    assert_eq!(tx_event["direction"], serde_json::json!("tx"));
    assert_eq!(tx_event["seq"], serde_json::json!(1));

    // 设备侧发来字节 → onData（尾部静默窗口后成段）。
    transport.feed(b"pong");
    let event = tokio::time::timeout(Duration::from_secs(2), data_events.recv())
        .await
        .expect("应收到 onData")
        .expect("流未关闭");
    assert_eq!(event["path"], serde_json::json!("/dev/ttyS0"));
    assert_eq!(event["direction"], serde_json::json!("rx"));
    assert_eq!(event["bytes"], serde_json::json!([112, 111, 110, 103]));

    let snapshot = service.get_snapshot("/dev/ttyS0");
    assert_eq!(snapshot["stats"]["txBytes"], serde_json::json!(4));
    assert_eq!(snapshot["stats"]["rxBytes"], serde_json::json!(4));

    // 空写回当前 seq（不新增 chunk、不触达设备）。
    let seq = service
        .write("/dev/ttyS0", Vec::new(), "user", None)
        .await
        .unwrap();
    assert_eq!(seq, 2, "空写返回当前 lastSeq");
    assert_eq!(transport.writes.lock().unwrap().len(), 1);

    // 超过 64KB 被拒。
    let error = service
        .write("/dev/ttyS0", vec![0_u8; 64 * 1024 + 1], "user", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalidInput");

    // clear 只清缓冲，不动端口状态。
    service.clear("/dev/ttyS0");
    let snapshot = service.get_snapshot("/dev/ttyS0");
    assert_eq!(snapshot["chunks"], serde_json::json!([]));
    assert_eq!(snapshot["status"]["state"], serde_json::json!("open"));
    service.shutdown();
}

/// 未打开的路径写入 → notOpen；未知路径 clear 是空操作。
#[tokio::test]
async fn write_to_an_unopened_port_reports_not_open() {
    let fake = FakeEnumerator::new(vec![]);
    let (service, _transport) = data_service(&fake);
    let error = service
        .write("/dev/ttyS0", b"x".to_vec(), "user", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "notOpen");
    service.clear("/dev/nope");
    service.shutdown();
}
