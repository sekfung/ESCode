//! 循环发送的任务语义（`rust-u2-serial.md` §12.3）。
use super::*;
use std::sync::atomic::AtomicUsize;

fn counting_write(writes: Arc<AtomicUsize>) -> WriteFn {
    Arc::new(move |_bytes: Vec<u8>| {
        writes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { true })
    })
}

#[test]
fn loop_parameters_follow_the_typescript_validation() {
    assert!(is_valid_params(10, 1, None));
    assert!(!is_valid_params(9, 1, None), "间隔下限 10ms");
    assert!(!is_valid_params(10, 0, None), "空载荷");
    assert!(!is_valid_params(10, 1, Some(0)), "次数至少 1");
    assert!(is_valid_params(10, 1, Some(1)));
}

#[tokio::test]
async fn first_send_is_immediate_and_count_ends_the_loop() {
    let state = LoopState::new(1, 10, Some(1));
    let writes = Arc::new(AtomicUsize::new(0));
    let (finished_tx, mut finished_rx) = tokio::sync::mpsc::unbounded_channel();
    spawn(
        Arc::clone(&state),
        b"go".to_vec(),
        counting_write(Arc::clone(&writes)),
        Arc::new(|_| {}),
        Arc::new(move |generation| {
            let _ = finished_tx.send(generation);
        }),
    );
    let generation = tokio::time::timeout(Duration::from_secs(2), finished_rx.recv())
        .await
        .expect("立即发送并在 1 次后结束")
        .expect("回调通道有效");
    assert_eq!(generation, 1);
    assert_eq!(writes.load(Ordering::SeqCst), 1, "count=1 只写一次");
    assert_eq!(state.sent(), 1);
}

/// 上一笔写入未完成时跳过 tick，不排队积压。
#[tokio::test]
async fn ticks_are_skipped_while_a_write_is_in_flight() {
    let state = LoopState::new(1, 10, None);
    let writes = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Notify::new());
    let write: WriteFn = {
        let writes = Arc::clone(&writes);
        let gate = Arc::clone(&gate);
        Arc::new(move |_bytes: Vec<u8>| {
            writes.fetch_add(1, Ordering::SeqCst);
            let gate = Arc::clone(&gate);
            Box::pin(async move {
                gate.notified().await;
                true
            })
        })
    };
    spawn(
        Arc::clone(&state),
        b"x".to_vec(),
        write,
        Arc::new(|_| {}),
        Arc::new(|_| {}),
    );
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        1,
        "写入未完成时不能叠加第二次"
    );
    gate.notify_one();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        writes.load(Ordering::SeqCst) >= 2,
        "写入完成后继续按间隔发送"
    );
    state.stop();
}

/// 进度事件受 250ms 节流（启动事件由会话在 spawn 前单独发）。
#[tokio::test]
async fn progress_events_are_throttled() {
    let state = LoopState::new(1, 10, None);
    let progress = Arc::new(AtomicUsize::new(0));
    let progress_for_task = Arc::clone(&progress);
    spawn(
        Arc::clone(&state),
        b"x".to_vec(),
        counting_write(Arc::new(AtomicUsize::new(0))),
        Arc::new(move |_| {
            progress_for_task.fetch_add(1, Ordering::SeqCst);
        }),
        Arc::new(|_| {}),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(progress.load(Ordering::SeqCst), 0, "100ms 内不应有进度事件");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while progress.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "250ms 后应至少有一条进度事件"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    state.stop();
}

/// 停止后不再有写入。
#[tokio::test]
async fn stopping_ends_the_task() {
    let state = LoopState::new(1, 10, None);
    let writes = Arc::new(AtomicUsize::new(0));
    spawn(
        Arc::clone(&state),
        b"x".to_vec(),
        counting_write(Arc::clone(&writes)),
        Arc::new(|_| {}),
        Arc::new(|_| {}),
    );
    tokio::time::sleep(Duration::from_millis(40)).await;
    state.stop();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let after_stop = writes.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        after_stop,
        "stop 之后不能再写"
    );
}
