use super::*;

#[test]
fn hub_routes_progress_by_operation_id_and_drops_dead_listeners() {
    let hub = Hub::default();
    let mut mine = hub.listen("op-1");
    let other = hub.listen("op-2");
    drop(other);
    hub.dispatch(r#"{"method":"plugins/operationProgress","params":{"operationId":"op-1","phase":"download"}}"#);
    hub.dispatch(
        r#"{"method":"plugins/operationProgress","params":{"operationId":"op-2","phase":"x"}}"#,
    );
    hub.dispatch(r#"{"method":"other/notification","params":{"operationId":"op-1"}}"#);
    hub.dispatch("not json");
    assert_eq!(mine.try_recv().expect("progress")["phase"], "download");
    assert!(mine.try_recv().is_err(), "只收到属于自己的一条");
    assert!(
        lock(&hub.listeners).get("op-2").is_none(),
        "已断开的监听者被清理"
    );
}

fn timeout() -> CallError {
    CallError::Timeout(RpcError::new("Fatal", "runtime did not answer"))
}

#[tokio::test]
async fn a_stale_runtime_is_recycled_and_the_request_retried_once() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let recycled = std::sync::atomic::AtomicUsize::new(0);
    let result = with_stale_retry(
        "plugins/list",
        || async {
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(timeout())
            } else {
                Ok(serde_json::json!({"plugins": []}))
            }
        },
        || async {
            recycled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    )
    .await;
    assert_eq!(result.unwrap(), serde_json::json!({"plugins": []}));
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        recycled.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "只回收一次，且在重试之前"
    );
}

#[tokio::test]
async fn a_second_timeout_is_reported_without_a_third_attempt() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let error = with_stale_retry(
        "plugins/list",
        || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(timeout())
        },
        || async {},
    )
    .await
    .unwrap_err();
    assert_eq!(error.message, "runtime did not answer");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn non_timeout_failures_are_never_retried_or_recycled() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let recycled = std::sync::atomic::AtomicUsize::new(0);
    let error = with_stale_retry(
        "plugins/list",
        || async {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(CallError::Failed(RpcError::new(
                "ESCodeProtocolClientError",
                "boom",
            )))
        },
        || async {
            recycled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.name, "ESCodeProtocolClientError");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(recycled.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn only_the_read_only_list_request_opts_into_stale_recovery() {
    let retrying: Vec<&str> = SPECS
        .iter()
        .filter(|spec| spec.retry_stale)
        .map(|spec| spec.service)
        .collect();
    assert_eq!(
        retrying,
        ["listPlugins"],
        "写操作（安装/卸载…）重试会重复副作用，不能开启"
    );
}
