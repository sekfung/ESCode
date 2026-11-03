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

#[test]
fn changed_path_requires_exactly_one_known_path() {
    let single = BTreeSet::from(["a.txt".to_owned()]);
    assert_eq!(
        selected_changed_path(&single, false),
        Some("a.txt".to_owned())
    );
    assert_eq!(selected_changed_path(&single, true), None);

    let batch = BTreeSet::from(["a.txt".to_owned(), "b.txt".to_owned()]);
    assert_eq!(selected_changed_path(&batch, false), None);
}

#[tokio::test]
async fn single_changed_path_is_reported_and_batches_drop_it() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().to_string_lossy().into_owned();
    // 先建文件再监听，避免把「创建 + 写入」合并进同一窗口；macOS 的额外平台事件在下方的
    // changedPath 断言里单独容忍。
    tokio::fs::write(dir.path().join("a.txt"), "0")
        .await
        .expect("seed");
    let service = FileWatcherService::with_debounce(WINDOW);
    let id = service.watch(&root, false).await.expect("watch");
    assert_eq!(id, "0");
    let mut rx = service.listen(&id).expect("listen");

    tokio::fs::write(dir.path().join("a.txt"), "1")
        .await
        .expect("write");
    let event = next(&mut rx).await.expect("event");
    assert_eq!(event["dirPath"], root);
    let expected = dir.path().join("a.txt").to_string_lossy().into_owned();
    // macOS 的 FSEvents 在 CI 负载下仍可能把同一次写入拆出无法归属根路径的事件（rescan / 目录自身），
    // 产品按规格省略 changedPath。精确单路径选择由上面的纯函数用例覆盖，这里只容忍这条平台噪声。
    if cfg!(target_os = "macos") {
        let changed = &event["changedPath"];
        assert!(
            changed.is_null() || changed.as_str() == Some(expected.as_str()),
            "FSEvents returned an unexpected changedPath: event={event}"
        );
    } else {
        assert_eq!(event["changedPath"], expected.as_str());
    }
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
