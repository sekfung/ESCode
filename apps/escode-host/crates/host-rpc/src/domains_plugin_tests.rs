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
