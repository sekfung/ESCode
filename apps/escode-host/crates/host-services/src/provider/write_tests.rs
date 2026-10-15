//! Personal 写入面的并发、失败保护与权限（TS 差分覆盖不到的 Host 侧性质）。

use std::sync::Arc;

use serde_json::{Value, json};

use super::service_tests::builtin;
use super::*;

async fn service_in(dir: &std::path::Path) -> (Arc<ProviderService>, PathBuf) {
    let (builtin_path, personal_path) = (dir.join("b.json"), dir.join("p.json"));
    tokio::fs::write(&builtin_path, builtin().to_string())
        .await
        .expect("write builtin");
    (
        Arc::new(ProviderService::new(builtin_path, personal_path.clone())),
        personal_path,
    )
}

async fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_str(&tokio::fs::read_to_string(path).await.expect("read")).expect("json")
}

#[tokio::test]
async fn concurrent_creates_never_lose_an_update() {
    let dir = tempfile::tempdir().expect("dir");
    let (service, personal) = service_in(dir.path()).await;
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let service = Arc::clone(&service);
            tokio::spawn(async move { service.create_personal_provider(None).await })
        })
        .collect();
    for handle in handles {
        handle.await.expect("join").expect("create");
    }
    let file = read_json(&personal).await;
    let rules = file["config"]["providerConfigRules"]["providerRules"]
        .as_array()
        .expect("rules");
    assert_eq!(rules.len(), 8);
    let ids: std::collections::BTreeSet<_> = rules
        .iter()
        .map(|r| r["providerId"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(ids.len(), 8, "每个 Provider ID 唯一");
    assert_eq!(
        file["config"]["providerOrder"].as_array().map(Vec::len),
        Some(8)
    );
}

#[tokio::test]
async fn an_invalid_personal_file_is_never_overwritten() {
    let dir = tempfile::tempdir().expect("dir");
    let (service, personal) = service_in(dir.path()).await;
    tokio::fs::write(&personal, "{ not json")
        .await
        .expect("write");
    let error = service
        .create_personal_provider(None)
        .await
        .expect_err("invalid file");
    assert!(error.to_string().contains("JSON"), "{error:#}");
    assert_eq!(
        tokio::fs::read_to_string(&personal).await.expect("read"),
        "{ not json",
        "写入路径不得把用户的坏文件覆盖成空配置"
    );
}

#[tokio::test]
async fn renaming_to_the_same_id_does_not_touch_the_file() {
    let dir = tempfile::tempdir().expect("dir");
    let (service, personal) = service_in(dir.path()).await;
    service
        .rename_personal_model("b", "m", "m")
        .await
        .expect("noop rename");
    assert!(!personal.exists(), "同名重命名是无操作，不创建文件");
}

#[cfg(unix)]
#[tokio::test]
async fn written_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("dir");
    let (service, personal) = service_in(dir.path()).await;
    service
        .create_personal_provider(None)
        .await
        .expect("create");
    let mode = std::fs::metadata(&personal)
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    // 锁目录与临时文件不残留。
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .expect("dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".lock") || name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn a_write_is_visible_to_the_very_next_view() {
    let dir = tempfile::tempdir().expect("dir");
    let (service, _) = service_in(dir.path()).await;
    let before = service.provider_settings_view().await.expect("view");
    let created = service
        .create_personal_provider(Some(&json!({"providerName": "Fresh"})))
        .await
        .expect("create");
    let after = service.provider_settings_view().await.expect("view");
    assert!(after["revision"].as_u64() > before["revision"].as_u64());
    assert_eq!(created["view"]["revision"], after["revision"]);
}

#[test]
fn a_stale_membership_revision_is_rejected() {
    let doc = personal::Doc::empty();
    let membership = mutations::Membership {
        provider_id: "p".into(),
        inherited: Vec::new(),
        personal_revision: "stale".into(),
    };
    let error = mutations::assert_membership_current(&membership, "p", &doc).expect_err("stale");
    assert_eq!(error.0, "Provider Settings membership revision conflict");
}
