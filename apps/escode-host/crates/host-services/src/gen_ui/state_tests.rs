//! `gen-ui` 状态存储（`rust-u2-gen-ui.md` §3.2/§3.3/§6）。
use std::time::Duration;

use super::*;
use crate::gen_ui::schema::parse_scope;

fn target(path: &str) -> Value {
    serde_json::json!({"workspacePath": "/w", "sessionId": "s1", "path": path})
}

fn scope() -> Value {
    serde_json::json!({"workspacePath": "/w", "sessionId": "s1"})
}

fn store() -> (tempfile::TempDir, GenUiStateStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = GenUiStateStore::new(dir.path().join("gen-ui-state"));
    (dir, store)
}

fn scope_dir_of(store: &GenUiStateStore) -> PathBuf {
    scope_dir(&store.root, &parse_scope(&scope()).unwrap())
}

async fn record_names(store: &GenUiStateStore) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scope_dir_of(store))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    names.retain(|name| is_record_file_name(name));
    names.sort();
    names
}

#[tokio::test]
async fn set_then_get_round_trips_and_updated_at_is_monotonic() {
    let (_dir, store) = store();
    let path = "/root/scope/report.html";
    assert_eq!(
        store.get(&target(path)).await.unwrap(),
        Value::Null,
        "未写过即 null"
    );

    store
        .set(&serde_json::json!({"target": target(path), "state": {"modelContent": {"a": 1}}}))
        .await
        .unwrap();
    let state = store.get(&target(path)).await.unwrap();
    // 缺省键补 null（与 zod 的 `.default(null)` 同形）。
    assert_eq!(
        state,
        serde_json::json!({"modelContent": {"a": 1}, "privateContent": null})
    );

    // 同毫秒连写也要单调递增。
    let first = store
        .records(&parse_scope(&scope()).unwrap())
        .await
        .unwrap();
    store
        .set(&serde_json::json!({"target": target(path), "state": {"modelContent": 2}}))
        .await
        .unwrap();
    let second = store
        .records(&parse_scope(&scope()).unwrap())
        .await
        .unwrap();
    assert_eq!(first.len(), 1, "同一 target 只应有一条记录");
    assert_eq!(second.len(), 1);
    assert!(
        second[0].updated_at > first[0].updated_at,
        "updatedAt 必须严格递增: {} → {}",
        first[0].updated_at,
        second[0].updated_at
    );
}

#[tokio::test]
async fn capacity_eviction_keeps_the_newest_hundred() {
    let (_dir, store) = store();
    let dir = scope_dir_of(&store);
    std::fs::create_dir_all(&dir).unwrap();
    // 预置到上限的旧记录（不走过 100 次 set，避免测试退化成 O(n²)）。
    for index in 0..STATE_MAX_ENTRIES {
        let name = format!("{:064x}.json", index);
        let record = serde_json::json!({
            "target": target(&format!("/root/scope/old-{index}.html")),
            "state": {"modelContent": index},
            "updatedAt": index as f64,
        });
        std::fs::write(dir.join(name), serde_json::to_vec(&record).unwrap()).unwrap();
    }
    assert_eq!(record_names(&store).await.len(), STATE_MAX_ENTRIES);

    store
        .set(&serde_json::json!({"target": target("/root/scope/newest.html"), "state": {}}))
        .await
        .unwrap();
    let names = record_names(&store).await;
    assert_eq!(names.len(), STATE_MAX_ENTRIES, "淘汰后应恰好保留上限条数");
    // 最新的那条一定在；最旧的那条一定被删。
    assert!(names.contains(&format!("{}.json", hex_of("/root/scope/newest.html"))));
    assert!(
        !names.contains(&format!("{:064x}.json", 0)),
        "最旧的记录应被淘汰"
    );
}

/// 与实现同口径的文件名 hash（测试内用来定位刚写的记录，不校验算法本身）。
fn hex_of(path: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(path.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn list_filters_by_scope_and_sorts_by_updated_at_desc() {
    let (_dir, store) = store();
    // 插入序 = 时间序：older → middle → newer（后写的 updatedAt 更大）。
    for (path, content) in [
        ("/root/scope/older.html", 1),
        ("/root/scope/middle.html", 2),
        ("/root/scope/newer.html", 3),
    ] {
        store
            .set(&serde_json::json!({
                "target": target(path),
                "state": {"modelContent": content},
            }))
            .await
            .unwrap();
    }
    let entries = store.list(&scope()).await.unwrap();
    let paths: Vec<&str> = entries
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/root/scope/newer.html",
            "/root/scope/middle.html",
            "/root/scope/older.html"
        ],
        "按 updatedAt 降序"
    );

    // 另一个 session 的 scope 看不到这些记录（目录按 scope key 分）。
    let other = serde_json::json!({"workspacePath": "/w", "sessionId": "s2"});
    assert!(store.list(&other).await.unwrap().is_empty());
    // workspaceIdentity 参与 scope key：同 path 不同 identity 是不同 scope。
    let identified = serde_json::json!({
        "workspacePath": "/w", "workspaceIdentity": "ssh:host", "sessionId": "s1"
    });
    assert!(store.list(&identified).await.unwrap().is_empty());
}

#[tokio::test]
async fn notifications_dedupe_identical_state_and_report_deletion_as_null() {
    let (_dir, store) = store();
    let path = "/root/scope/report.html";
    let mut events = store.subscribe();

    store
        .set(&serde_json::json!({"target": target(path), "state": {"modelContent": 1}}))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("首次写入必须发事件")
        .expect("流未关闭");
    assert_eq!(event["target"]["path"], Value::from(path));
    assert_eq!(event["state"]["modelContent"], Value::from(1));

    // 同值再写：**不发**（去重）。
    store
        .set(&serde_json::json!({"target": target(path), "state": {"modelContent": 1}}))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), events.recv())
            .await
            .is_err(),
        "同值写入不应重复发事件"
    );

    // 模型侧直接改文件（不经 setState）→ watcher 通知。
    let file = scope_dir_of(&store).join(format!("{}.json", hex_of(path)));
    let record = serde_json::json!({
        "target": target(path),
        "state": {"modelContent": 7, "privateContent": null},
        "updatedAt": 9_999_999_999.0_f64,
    });
    std::fs::write(&file, serde_json::to_vec(&record).unwrap()).unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("文件变更必须发事件")
        .expect("流未关闭");
    assert_eq!(event["state"]["modelContent"], Value::from(7));

    // 记录被删 → 事件带 `state: null`。
    std::fs::remove_file(&file).unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("删除必须发事件")
        .expect("流未关闭");
    assert_eq!(event["state"], Value::Null);
}

#[tokio::test]
async fn corrupt_records_fail_the_read_but_missing_ones_are_skipped() {
    let (_dir, store) = store();
    let dir = scope_dir_of(&store);
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("{:064x}.json", 1);
    // 坏 JSON：`records` 整体失败（TS 的 `Promise.all` 同——不静默跳过）。
    std::fs::write(dir.join(&name), b"{not json").unwrap();
    assert!(store.list(&scope()).await.is_err());
    std::fs::remove_file(dir.join(&name)).unwrap();
    // 超大记录同样判无效（> 96 KiB）。
    let big = format!("{:064x}.json", 2);
    let record = serde_json::json!({
        "target": target("/root/scope/big.html"),
        "state": {"modelContent": "x".repeat(RECORD_MAX_BYTES as usize)},
        "updatedAt": 1,
    });
    std::fs::write(dir.join(&big), serde_json::to_vec(&record).unwrap()).unwrap();
    let error = store.list(&scope()).await.unwrap_err();
    assert!(error.contains("record size"), "{error}");

    // 目录里没有记录时回空列表（缺目录也算）。
    std::fs::remove_file(dir.join(&big)).unwrap();
    assert!(store.list(&scope()).await.unwrap().is_empty());
}
