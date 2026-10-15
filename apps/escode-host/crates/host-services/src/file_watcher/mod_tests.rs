//! 防抖与事件投递单测（真实文件系统，缩短防抖窗口）。

use std::time::Duration;

use super::*;

const WINDOW: Duration = Duration::from_millis(40);

async fn next(rx: &mut mpsc::UnboundedReceiver<Value>) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .ok()
        .flatten()
}

async fn quiet(rx: &mut mpsc::UnboundedReceiver<Value>) -> bool {
    tokio::time::timeout(WINDOW * 8, rx.recv()).await.is_err()
}

#[tokio::test]
async fn single_changed_path_is_reported_and_batches_drop_it() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().to_string_lossy().into_owned();
    let service = FileWatcherService::with_debounce(WINDOW);
    let id = service.watch(&root, false).await.expect("watch");
    assert_eq!(id, "0");
    let mut rx = service.listen(&id).expect("listen");

    tokio::fs::write(dir.path().join("a.txt"), "1")
        .await
        .expect("write");
    let event = next(&mut rx).await.expect("event");
    assert_eq!(event["dirPath"], root);
    assert_eq!(
        event["changedPath"],
        dir.path().join("a.txt").to_string_lossy().as_ref()
    );
    assert!(quiet(&mut rx).await, "同一次写入的多个底层事件合并为一个");

    tokio::fs::write(dir.path().join("b.txt"), "1")
        .await
        .expect("write");
    tokio::fs::write(dir.path().join("c.txt"), "1")
        .await
        .expect("write");
    let batch = next(&mut rx).await.expect("batch");
    assert!(
        batch.get("changedPath").is_none(),
        "窗口内多个路径不带 changedPath"
    );
}

#[tokio::test]
async fn unwatch_cancels_pending_event_and_unknown_ids_are_harmless() {
    let dir = tempfile::tempdir().expect("dir");
    let service = FileWatcherService::with_debounce(WINDOW);
    let id = service
        .watch(&dir.path().to_string_lossy(), false)
        .await
        .expect("watch");
    let mut rx = service.listen(&id).expect("listen");
    tokio::fs::write(dir.path().join("a.txt"), "1")
        .await
        .expect("write");
    service.unwatch(&id);
    assert!(
        next(&mut rx).await.is_none(),
        "unwatch 之后流结束，待发事件被取消"
    );
    assert!(service.listen(&id).is_none());
    service.unwatch("no-such-id");
    assert_eq!(
        service
            .watch(&dir.path().to_string_lossy(), true)
            .await
            .expect("watch"),
        "1"
    );
    service.dispose_all();
    assert!(service.listen("1").is_none());
}

#[tokio::test]
async fn missing_directory_reports_node_style_error() {
    let service = FileWatcherService::with_debounce(WINDOW);
    let error = service
        .watch("/definitely/not/here", false)
        .await
        .expect_err("error");
    assert_eq!(
        error.to_string(),
        "无法监视目录 '/definitely/not/here': ENOENT: no such file or directory, watch '/definitely/not/here'"
    );
    // 失败也消耗 id（TS 先 nextId++）。
    let dir = tempfile::tempdir().expect("dir");
    assert_eq!(
        service
            .watch(&dir.path().to_string_lossy(), false)
            .await
            .expect("watch"),
        "1"
    );
}
