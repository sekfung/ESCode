//! 信号与循环发送（`rust-u2-serial.md` §12）。
use super::*;
use crate::serial::signals::Pulse;

// ── S4：信号 ──────────────────────────────────────────────────────────────

fn signal_log(transport: &FakeTransport) -> Vec<Signals> {
    transport.signals.lock().unwrap().clone()
}

#[tokio::test]
async fn set_signals_applies_the_requested_lines_and_broadcasts_status() {
    let (transport, session, events) = make_session();
    session.open(config(false)).await.unwrap();

    let applied = session.set_signals(Some(false), None, None).await.unwrap();
    assert_eq!(
        applied,
        Signals {
            dtr: false,
            rts: true
        },
        "只改 DTR，RTS 保持默认"
    );
    assert_eq!(signal_log(&transport), vec![applied]);

    let applied = session.set_signals(None, Some(false), None).await.unwrap();
    assert_eq!(
        applied,
        Signals {
            dtr: false,
            rts: false
        }
    );
    let last = events.lock().unwrap().last().cloned().expect("状态事件");
    assert_eq!(
        last["signals"],
        serde_json::json!({"dtr": false, "rts": false})
    );
    assert_eq!(last["state"], serde_json::json!("open"));
    assert_eq!(session.current_status()["signals"], last["signals"]);
}

#[tokio::test]
async fn set_signals_rejects_explicit_rts_under_flow_control_but_allows_pulses() {
    let (transport, session, _events) = make_session();
    session.open(config_with(true, false)).await.unwrap();

    let error = session
        .set_signals(None, Some(false), None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalidInput");
    assert_eq!(error.message, "RTS is controlled by RTS/CTS flow control");
    assert!(signal_log(&transport).is_empty(), "被拒绝时不能改线");

    // TS 只检查显式 rts 字段：脉冲在 rtscts 下仍会执行。
    session
        .set_signals(None, None, Some(Pulse::Arduino))
        .await
        .unwrap();
    assert_eq!(
        signal_log(&transport),
        vec![
            Signals {
                dtr: true,
                rts: true
            },
            Signals {
                dtr: false,
                rts: true
            },
            Signals {
                dtr: true,
                rts: true
            },
        ],
        "Arduino 两步 + 恢复脉冲前状态"
    );
}

/// ESP32 脉冲：三步 + 恢复脉冲前状态；100ms/50ms 的保持时间真实生效。
#[tokio::test]
async fn esp32_pulse_holds_each_step_then_restores_the_previous_signals() {
    let (transport, session, _events) = make_session();
    session.open(config(false)).await.unwrap();
    session
        .set_signals(Some(false), Some(true), None)
        .await
        .unwrap();

    let started = tokio::time::Instant::now();
    let applied = session
        .set_signals(None, None, Some(Pulse::Esp32))
        .await
        .unwrap();
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(140),
        "100ms + 50ms 的保持时间不能省"
    );
    assert_eq!(
        applied,
        Signals {
            dtr: false,
            rts: true
        },
        "脉冲结束恢复脉冲前的状态"
    );
    assert_eq!(
        signal_log(&transport),
        vec![
            Signals {
                dtr: false,
                rts: true
            },
            Signals {
                dtr: false,
                rts: true
            },
            Signals {
                dtr: true,
                rts: false
            },
            Signals {
                dtr: false,
                rts: false
            },
            Signals {
                dtr: false,
                rts: true
            },
        ],
        "手动设置 → 三步脉冲 → 恢复"
    );
}

/// setSignals 与脉冲同队列：脉冲进行中的调用排在其后，不会交错。
#[tokio::test]
async fn set_signals_waits_for_an_in_flight_pulse() {
    let (transport, session, _events) = make_session();
    session.open(config(false)).await.unwrap();

    let pulse = session.set_signals(None, None, Some(Pulse::Esp32));
    let plain = session.set_signals(Some(false), Some(false), None);
    let (pulse, plain) = tokio::join!(pulse, plain);
    pulse.unwrap();
    plain.unwrap();
    assert_eq!(
        signal_log(&transport).last().copied(),
        Some(Signals {
            dtr: false,
            rts: false
        }),
        "后一个调用在脉冲与恢复之后才生效"
    );
    assert_eq!(signal_log(&transport).len(), 5, "三步 + 恢复 + 后一个调用");
}

/// 用户 open 重置为默认信号；自动重连恢复断开前的状态。
#[tokio::test]
async fn open_resets_signals_and_reconnect_restores_them() {
    let (transport, session, _events) = make_session();
    session.open(config(true)).await.unwrap();
    session
        .set_signals(Some(false), Some(true), None)
        .await
        .unwrap();
    session.close().await;
    assert_eq!(
        session.current_status()["signals"],
        serde_json::json!({"dtr": false, "rts": true}),
        "关闭不清除信号记录"
    );
    session.open(config(true)).await.unwrap();
    assert_eq!(
        session.current_status()["signals"],
        serde_json::json!({"dtr": true, "rts": true}),
        "用户主动打开回到默认值"
    );

    // 自动重连：断开前的非默认状态在重开后恢复。
    session
        .set_signals(Some(false), Some(false), None)
        .await
        .unwrap();
    session.handle_disconnect().await;
    session.reconnect().await.unwrap();
    assert_eq!(
        transport.signals.lock().unwrap().last().copied(),
        Some(Signals {
            dtr: false,
            rts: false
        })
    );
    assert_eq!(
        session.current_status()["signals"],
        serde_json::json!({"dtr": false, "rts": false})
    );
}

// ── S4：循环发送 ──────────────────────────────────────────────────────────

async fn wait_for_writes(transport: &FakeTransport, expected: usize) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while transport.writes.lock().unwrap().len() < expected {
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待第 {expected} 次写入超时"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn start_loop_sends_immediately_reports_progress_and_stops_at_count() {
    let (transport, session, events) = make_session();
    session.open(config(false)).await.unwrap();
    session.start_loop(b"tick".to_vec(), 20, Some(2)).unwrap();

    // 启动事件带 loop（sent=0），不等待第一个 tick。
    let started = events.lock().unwrap().last().cloned().expect("状态事件");
    assert_eq!(started["loop"]["intervalMs"], serde_json::json!(20));
    assert_eq!(started["loop"]["count"], serde_json::json!(2));
    assert_eq!(started["loop"]["sent"], serde_json::json!(0));

    wait_for_writes(&transport, 2).await;
    assert_eq!(
        transport.writes.lock().unwrap().as_slice(),
        &[b"tick".to_vec(), b"tick".to_vec()]
    );

    // 达到次数后任务结束：状态不再带 loop。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let status = session.current_status();
        if status.get("loop").is_none() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "count 用完后 loop 应消失: {status}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(transport.writes.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn start_loop_validates_parameters_and_open_state() {
    let (_transport, session, _events) = make_session();
    let error = session.start_loop(b"x".to_vec(), 10, None).unwrap_err();
    assert_eq!(error.code, "notOpen");

    session.open(config(false)).await.unwrap();
    for (bytes, interval, count) in [
        (b"".to_vec(), 10, None),
        (b"x".to_vec(), 9, None),
        (b"x".to_vec(), 10, Some(0)),
        (vec![0_u8; WRITE_LIMIT_BYTES + 1], 10, None),
    ] {
        let error = session.start_loop(bytes, interval, count).unwrap_err();
        assert_eq!(error.code, "invalidInput");
        assert_eq!(error.message, "Invalid loop parameters");
    }
}

#[tokio::test]
async fn start_loop_replaces_the_previous_runner() {
    let (transport, session, _events) = make_session();
    session.open(config(false)).await.unwrap();
    session.start_loop(b"old".to_vec(), 200, None).unwrap();
    wait_for_writes(&transport, 1).await;

    session.start_loop(b"new".to_vec(), 20, Some(1)).unwrap();
    wait_for_writes(&transport, 2).await;
    let after_replace = transport
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|bytes| bytes.as_slice() == b"old")
        .count();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        transport
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|bytes| bytes.as_slice() == b"old")
            .count(),
        after_replace,
        "旧任务停止后不能再写"
    );
    assert_eq!(
        transport.writes.lock().unwrap().last().map(Vec::as_slice),
        Some(b"new".as_slice())
    );
}

#[tokio::test]
async fn stop_loop_is_idempotent_and_close_stops_the_loop() {
    let (transport, session, events) = make_session();
    session.open(config(false)).await.unwrap();
    let before = events.lock().unwrap().len();
    session.stop_loop();
    assert_eq!(
        events.lock().unwrap().len(),
        before,
        "没有循环时 stopLoop 不发事件"
    );

    session.start_loop(b"x".to_vec(), 20, None).unwrap();
    wait_for_writes(&transport, 1).await;
    session.stop_loop();
    assert!(
        session.current_status().get("loop").is_none(),
        "stopLoop 后状态不带 loop"
    );
    let stopped_at = transport.writes.lock().unwrap().len();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(
        transport.writes.lock().unwrap().len(),
        stopped_at,
        "stopLoop 之后不再写"
    );

    // 关闭会话同样自动停循环。
    session.start_loop(b"y".to_vec(), 20, None).unwrap();
    wait_for_writes(&transport, stopped_at + 1).await;
    session.close().await;
    let closed_at = transport.writes.lock().unwrap().len();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(
        transport.writes.lock().unwrap().len(),
        closed_at,
        "关闭后循环必须停止"
    );
    let status = session.current_status();
    assert_eq!(status["state"], serde_json::json!("closed"));
    assert!(status.get("loop").is_none());
}

/// 关闭时必须**立即**清掉 loop，而不是等下一个 tick（长间隔下差别可见）。
#[tokio::test]
async fn closing_clears_a_long_interval_loop_immediately() {
    let (transport, session, _events) = make_session();
    session.open(config(false)).await.unwrap();
    session.start_loop(b"x".to_vec(), 5000, None).unwrap();
    wait_for_writes(&transport, 1).await;
    session.close().await;
    let status = session.current_status();
    assert_eq!(status["state"], serde_json::json!("closed"));
    assert!(
        status.get("loop").is_none(),
        "关闭后状态立即不带 loop: {status}"
    );
}
