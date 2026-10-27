//! Registry 刷新与文件指纹闸门（P1 #7）：未变配置零真读、变更可发现、账号旁路。
use super::*;
use sha2::Digest;

fn fixture_config(revision: u64) -> String {
    format!(
        "{{\"schemaVersion\":1,\"revision\":{revision},\"config\":{{\"providerConfigRules\":{{\"templateRules\":[],\"providerRules\":[]}}}}}}"
    )
}

fn fixture_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let builtin = dir.path().join("builtin.json");
    let personal = dir.path().join("personal.json");
    std::fs::write(&builtin, fixture_config(1)).unwrap();
    std::fs::write(
        &personal,
        "{\"schemaVersion\":1,\"config\":{\"providerConfigRules\":{}}}",
    )
    .unwrap();
    (dir, builtin, personal)
}

#[tokio::test]
async fn unchanged_config_is_not_read_again_within_the_backstop() {
    let (_dir, builtin, personal) = fixture_dir();
    let registry = Registry::new(builtin.clone(), personal);
    assert!(registry.refresh(None).await.unwrap(), "首次刷新建快照");
    assert!(
        !registry.refresh(None).await.unwrap(),
        "指纹未变的第二次刷新必须跳过真读并报告无变化"
    );
}

#[tokio::test]
async fn config_change_is_detected_after_the_gate() {
    let (_dir, builtin, personal) = fixture_dir();
    let registry = Registry::new(builtin.clone(), personal);
    assert!(registry.refresh(None).await.unwrap());
    assert!(!registry.refresh(None).await.unwrap());
    std::fs::write(&builtin, fixture_config(2)).unwrap();
    assert!(
        registry.refresh(None).await.unwrap(),
        "配置改写必须被指纹发现并重建快照"
    );
}

#[tokio::test]
async fn backstop_reads_even_when_the_stamp_is_unchanged() {
    let (_dir, builtin, personal) = fixture_dir();
    let registry = Registry::new(builtin, personal);
    // 兜底间隔置零：每轮都真读——等价于没有闸门，验证 record 的时间语义。
    *registry.gate.lock().await =
        escode_cli_host::file_stamp::ChangeGate::new(std::time::Duration::ZERO);
    assert!(registry.refresh(None).await.unwrap());
    assert!(
        !registry.refresh(None).await.unwrap(),
        "真读后内容签名相同仍报告无变化"
    );
}

#[tokio::test]
async fn account_update_bypasses_the_gate() {
    let (_dir, builtin, personal) = fixture_dir();
    let registry = Registry::new(builtin.clone(), personal);
    assert!(registry.refresh(None).await.unwrap());
    assert!(!registry.refresh(None).await.unwrap());
    // 账号 overlay 必须锚定当前 builtin 修订（`escode-builtin:{revision}:{path sha}`）。
    let source = format!(
        "{:x}",
        sha2::Sha256::digest(
            std::path::absolute(&builtin)
                .unwrap()
                .to_string_lossy()
                .as_bytes()
        )
    );
    let account = serde_json::json!({
        "revision": "1",
        "basedOnESCodeBuiltinRevision": format!("escode-builtin:1:{source}"),
        "providers": {},
    });
    assert!(
        registry.refresh(Some(account)).await.unwrap(),
        "账号事实变化必须绕过闸门并重建快照"
    );
    assert!(
        !registry.refresh(None).await.unwrap(),
        "账号更新后的下一轮回到闸门语义"
    );
}

#[tokio::test]
async fn shared_registry_reuses_one_snapshot_per_path_pair() {
    let (_dir, builtin, personal) = fixture_dir();
    let first = Registry::shared(builtin.clone(), personal.clone())
        .await
        .unwrap();
    let second = Registry::shared(builtin.clone(), personal.clone())
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second), "同一路径对必须复用同一份快照");

    // 另一组路径（不同临时目录）不能被复用：测试与多环境隔离依赖这一点。
    let (_other_dir, other_builtin, other_personal) = fixture_dir();
    let other = Registry::shared(other_builtin, other_personal)
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &other), "不同路径对必须各自独立");
}

#[tokio::test]
async fn shared_registry_is_recreated_after_all_owners_drop() {
    let (_dir, builtin, personal) = fixture_dir();
    let first = Registry::shared(builtin.clone(), personal.clone())
        .await
        .unwrap();
    let weak = Arc::downgrade(&first);
    drop(first);
    assert!(weak.upgrade().is_none(), "最后一个实例退出后应可回收");

    let second = Registry::shared(builtin, personal).await.unwrap();
    assert!(weak.upgrade().is_none(), "缓存不得复活已回收的旧快照");
    assert!(Arc::strong_count(&second) >= 1);
}

#[tokio::test]
async fn shared_registry_account_is_visible_to_all_instances() {
    let (_dir, builtin, personal) = fixture_dir();
    let first = Registry::shared(builtin.clone(), personal.clone())
        .await
        .unwrap();
    let second = Registry::shared(builtin.clone(), personal).await.unwrap();
    let source = format!(
        "{:x}",
        sha2::Sha256::digest(
            std::path::absolute(&builtin)
                .unwrap()
                .to_string_lossy()
                .as_bytes()
        )
    );
    let account = serde_json::json!({
        "revision": "1",
        "basedOnESCodeBuiltinRevision": format!("escode-builtin:1:{source}"),
        "providers": {},
    });
    assert!(
        first.refresh(Some(account.clone())).await.unwrap(),
        "账号事实变化必须重建共享快照"
    );
    assert_eq!(
        second.received_account().await,
        Some(account),
        "共享实例必须看到同一账号事实"
    );
}
