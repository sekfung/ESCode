use std::sync::Arc;
use std::time::Duration;

use super::{DirtyFlags, LocalScope, WindowController};
use crate::domains_task_support::{LazyIndex, ReadContext};
use crate::task_index_syncer::TaskIndexSyncer;
use crate::workspace_events::WorkspaceEvents;

/// 测试用索引：库文件 + 迁移账本（`TasksIndex::open` 只校验不建库）。
fn prepare_index(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    escode_host_services::tasks_index::fixture::apply_generated_schema(
        &connection,
        escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS,
    )
    .unwrap();
}

fn controller_fixture(index_path: std::path::PathBuf) -> Arc<WindowController> {
    let read = ReadContext {
        path: index_path,
        index: Arc::new(std::sync::Mutex::new(LazyIndex::default())),
        targets: Arc::new(crate::task_targets::TaskTargets::new()),
        runtime: None,
    };
    let events = Arc::new(WorkspaceEvents::default());
    let syncer = Arc::new(TaskIndexSyncer::empty_for_test(Arc::clone(&events)));
    let commands = Arc::new(crate::task_commands::TaskCommandQueue::new());
    let task_events = Arc::new(crate::task_events::TaskEventHub::default());
    WindowController::new(
        read,
        events,
        syncer,
        commands,
        task_events,
        std::path::PathBuf::from("/tmp"),
    )
}

/// 取消路径（评审遗留）：owner 释放后两个监听任务必须真的停表，而不是继续挂在
/// `recv()` 上等下一次事件（`Weak` 只在事件到达时才判活，不是取消）。
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_controller_aborts_its_watcher_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let index_path = temp.path().join("tasks-index.sqlite");
    let prepare_path = index_path.clone();
    tokio::task::spawn_blocking(move || prepare_index(&prepare_path))
        .await
        .unwrap();
    let controller = controller_fixture(index_path);
    let scope = LocalScope::new("/workspace/alpha", None);
    controller.refresh(&scope, true).await.unwrap();
    // 每 source 的索引写监听 + syncer 摘要监听。
    let tasks = controller.watcher_abort_handles();
    assert_eq!(tasks.len(), 2, "注册 source 后应有两个监听任务");

    drop(controller);

    // abort 是协作式的：让出后任务才会看到取消信号。
    let settled = tokio::time::timeout(Duration::from_secs(2), async {
        while !tasks.iter().all(tokio::task::AbortHandle::is_finished) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(settled.is_ok(), "drop 后监听任务必须收尾：{tasks:?}");
}

#[test]
fn signals_during_a_read_survive_and_failed_reads_keep_the_flag() {
    let flags = DirtyFlags::default();
    flags.mark("s");
    assert!(flags.is_dirty("s"));
    flags.begin_read("s");
    assert!(!flags.is_dirty("s"), "读库前清标记");
    flags.mark("s");
    assert!(flags.is_dirty("s"), "读库期间到达的信号留下标记 → 续跑");
    flags.begin_read("s");
    flags.restore("s");
    assert!(flags.is_dirty("s"), "读库失败 → 放回标记，下次重试");
    assert!(!flags.is_dirty("other"));
}
