use std::sync::Arc;
use std::time::Duration;

use super::super::service_tests::builtin;
use super::*;

async fn next_change(receiver: &mut watch::Receiver<u64>) -> Option<u64> {
    tokio::time::timeout(Duration::from_secs(5), receiver.changed())
        .await
        .ok()?
        .ok()?;
    Some(*receiver.borrow_and_update())
}

#[tokio::test]
async fn external_edits_and_own_writes_notify_but_the_initial_snapshot_does_not() {
    let dir = tempfile::tempdir().expect("dir");
    let (builtin_path, personal_path) = (dir.path().join("b.json"), dir.path().join("p.json"));
    tokio::fs::write(&builtin_path, builtin().to_string())
        .await
        .expect("write");
    let service = Arc::new(
        ProviderService::new(builtin_path, personal_path.clone())
            .with_poll_interval(Duration::from_millis(30)),
    );
    let mut receiver = service.watch_changes().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), receiver.changed())
            .await
            .is_err(),
        "订阅时的初始快照不算变化，空闲时不通知"
    );

    // 外部改文件：由轮询发现。
    tokio::fs::write(
        &personal_path,
        r#"{"schemaVersion":1,"config":{"providerOrder":["x"],"providerConfigRules":{"providerRules":[]},"modelConfigRules":{"providerModelRules":[],"manualProviderModelRules":[]}}}"#,
    )
    .await
    .expect("write");
    let external = next_change(&mut receiver).await.expect("external change");

    // 同内容重写：指纹变但内容不变，不通知。
    let text = tokio::fs::read_to_string(&personal_path)
        .await
        .expect("read");
    tokio::fs::write(&personal_path, format!("{text}\n"))
        .await
        .expect("write");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), receiver.changed())
            .await
            .is_err(),
        "内容未变不得通知"
    );

    // 本进程写入：同一条路径。
    service
        .create_personal_provider(None)
        .await
        .expect("create");
    let own = next_change(&mut receiver).await.expect("own write");
    assert!(own > external);
}
