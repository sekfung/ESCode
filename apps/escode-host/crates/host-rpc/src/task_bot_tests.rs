use std::sync::Arc;
use std::time::Duration;

use super::*;

#[tokio::test]
async fn invalidate_bumps_the_version_and_waits_for_the_inflight_send() {
    let state = Arc::new(BotGroupState::default());
    let (version, lock) = state.begin("k");
    assert!(state.is_current("k", version));
    let guard = lock.lock().await;
    let invalidating = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { state.invalidate("k").await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !state.is_current("k", version),
        "版本先推进，准备中的输入应失败"
    );
    assert!(!invalidating.is_finished(), "在途发送未收口前必须等待");
    drop(guard);
    invalidating.await.expect("invalidate completes");
    assert_eq!(state.capture("k"), version + 1);
}
