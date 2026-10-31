//! 远端 window-controller 写面与失败路径单测（spec §10.13 验收 2/3/5）。

use super::*;

/// 验收 2：写操作转调远端写面，刷新后的 membership 与远端一致。
#[tokio::test]
async fn mutations_forward_to_escode_task_and_refresh_membership() {
    let (controller, store, server) =
        controller_with(IDENTITY, vec![FakeTask::new("task-1", "Alpha", 10)]).await;
    let target = address(IDENTITY, SOURCE, "task-1");

    let meta = controller
        .mutate_task(&target, &json!({"kind": "pin", "pinned": true}))
        .await
        .expect("pin");
    assert_eq!(meta["taskId"], json!("task-1"));
    assert!(store.task("task-1").expect("stored task").pinned);
    let row = controller
        .projector()
        .row(&target, "task-1")
        .expect("projected row");
    assert_eq!(row["membership"]["pinned"], json!(true));

    controller
        .mutate_task(&target, &json!({"kind": "archive", "archived": true}))
        .await
        .expect("archive");
    let row = controller
        .projector()
        .row(&target, "task-1")
        .expect("projected row");
    assert_eq!(row["membership"]["archived"], json!(true));

    controller
        .mutate_task(&target, &json!({"kind": "mark-read"}))
        .await
        .expect("mark read");
    assert!(!store.task("task-1").expect("stored task").unread);
    server.abort();
}

/// 验收 3：远端 scope 只服务绑定的 workspace；其它 scope 的写入显式拒绝。
#[tokio::test]
async fn other_workspace_scopes_are_rejected() {
    let (controller, _store, server) =
        controller_with(IDENTITY, vec![FakeTask::new("task-1", "Alpha", 10)]).await;
    let foreign_identity = "remote:server:55555555-5555-4555-8555-555555555555:/remote/other";
    let foreign = address(foreign_identity, OTHER_SOURCE, "task-1");

    let error = controller
        .mutate_task(&foreign, &json!({"kind": "pin", "pinned": true}))
        .await
        .expect_err("foreign scope rejected");
    assert_eq!(error.message, "没有与任务地址匹配的 source");
    server.abort();
}

/// 验收 5：远端缺少 `escode-task` 时返回显式错误，不回落本机 DB、不返回伪造空列表。
#[tokio::test]
async fn missing_task_channel_is_an_explicit_error() {
    let (client, outbound) = initialized_client(&["window-project-changes"]);
    let server = spawn_unknown_channel_server(Arc::clone(&client), outbound);
    let controller = RemoteController::new(client, &scope(IDENTITY));

    let error = controller
        .list_task_list(json!({"kind": "active"}))
        .await
        .expect_err("missing escode-task must fail");
    assert!(
        error.message.contains("escode-task.listTasks failed"),
        "错误必须指向远端调用: {}",
        error.message
    );
    assert!(controller.projector().rows(&[]).is_empty());
    server.abort();
}

/// 搜索失败只记录并返回空，不清空可信投影。
#[tokio::test]
async fn search_failures_degrade_to_an_empty_page() {
    let (controller, store, server) =
        controller_with(IDENTITY, vec![FakeTask::new("task-1", "Alpha", 10)]).await;
    controller
        .list_task_list(json!({"kind": "active"}))
        .await
        .expect("initial refresh");
    *store.fail_search.lock().expect("fail flag") = true;

    let result = controller
        .list_task_list(json!({"kind": "active", "search": "alpha"}))
        .await
        .expect("search failure is logged, not propagated");
    assert_eq!(result["items"], json!([]));
    assert_eq!(result["total"], json!(0));
    assert!(!controller.projector().rows(&[]).is_empty(), "旧投影保留");
    server.abort();
}

/// 无法从 stable identity 解析路径时只允许读，写操作显式拒绝（spec §10.13）。
#[tokio::test]
async fn unparsable_identity_rejects_writes() {
    let (controller, _store, server) = controller_with(
        "/remote/workspace",
        vec![FakeTask::new("task-1", "Alpha", 10)],
    )
    .await;
    controller
        .list_task_list(json!({"kind": "active"}))
        .await
        .expect("reads still work");
    let target = address("/remote/workspace", SOURCE, "task-1");
    let error = controller
        .mutate_task(&target, &json!({"kind": "pin", "pinned": true}))
        .await
        .expect_err("write rejected");
    assert_eq!(
        error.message,
        "远端 workspace identity 无法解析，禁止列表写操作"
    );
    server.abort();
}
