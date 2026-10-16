//! Built-in Active/LKG 物化（spec §11）。路径向量由 TS `createESCodeBuiltinEndpointKey` 生成。

use serde_json::{Value, json};

use super::builtin_source::*;
use super::service_tests::builtin;

fn with_revision(revision: u64, extra_provider: Option<&str>) -> Value {
    let mut value = builtin();
    value["revision"] = json!(revision);
    if let Some(id) = extra_provider {
        value["config"]["providerConfigRules"]["providerRules"][0]["providerId"] = json!(id);
    }
    value
}

fn cache(root: &std::path::Path) -> ActiveCache {
    ActiveCache {
        root: root.to_path_buf(),
        app_version: "1.2.3".into(),
        endpoint: super::endpoint::EndpointSource {
            escode_env: Some("production".into()),
            base_origin: Some("https://api.example.com".into()),
            settings_home: None,
        },
    }
}

fn active_of(root: &std::path::Path) -> std::path::PathBuf {
    active_file_path(root, "1.2.3", "https://api.example.com", &client_platform()).unwrap()
}

async fn write(path: &std::path::Path, value: &Value) {
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, value.to_string()).await.unwrap();
}

#[test]
fn endpoint_key_matches_ts_vectors() {
    for (origin, key) in [
        (
            "https://api.example.com",
            "endpoint-137b9e5e4e13211ce3487cb1f3148ad0",
        ),
        (
            "https://API.Example.com:443/path?x=1",
            "endpoint-137b9e5e4e13211ce3487cb1f3148ad0",
        ),
        (
            "http://localhost:8080",
            "endpoint-a76d8c3e94eba61fc46b5cc05fc51e5f",
        ),
        (
            "http://127.0.0.1:3000/a",
            "endpoint-0398673719884a6295ed087fe9520379",
        ),
        (
            "https://例え.jp",
            "endpoint-372744ca99396c6e4ab3fc8832a0e9f3",
        ),
    ] {
        assert_eq!(endpoint_key(origin).unwrap(), key, "{origin}");
    }
    assert!(endpoint_key("  ").is_err());
    assert!(endpoint_key("ftp://x.example.com").is_err());
}

#[test]
fn active_path_matches_ts_and_rejects_bad_segments() {
    let root = std::path::Path::new("/cfg");
    let origin = "https://api.example.com";
    assert_eq!(
        active_file_path(root, "1.2.3", origin, "linux-x86_64")
            .unwrap()
            .to_string_lossy(),
        "/cfg/runtime/provider/linux-x86_64/1.2.3/endpoint-137b9e5e4e13211ce3487cb1f3148ad0/escode-builtin.json"
    );
    for bad in ["", ".", "..", "a/b", "a\\b"] {
        assert!(
            active_file_path(root, bad, origin, "linux-x86_64").is_err(),
            "{bad:?}"
        );
        assert!(
            active_file_path(root, "1.2.3", origin, bad).is_err(),
            "{bad:?}"
        );
    }
    assert_eq!(
        control_file_path(&active_file_path(root, "1.2.3", origin, "linux-x86_64").unwrap())
            .file_name()
            .unwrap(),
        "escode-builtin-refresh.json"
    );
}

#[tokio::test]
async fn materializes_bundled_into_missing_active() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(3, None)).await;
    let source = BuiltinSource::new(bundled, Some(cache(dir.path())));
    assert_eq!(source.read().await.unwrap()["revision"], 3);
    let active = active_of(dir.path());
    let written: Value =
        serde_json::from_str(&tokio::fs::read_to_string(active).await.unwrap()).unwrap();
    assert_eq!(written, with_revision(3, None));
}

#[tokio::test]
async fn newer_active_wins_and_is_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(1, None)).await;
    let active = active_of(dir.path());
    write(&active, &with_revision(5, Some("remote"))).await;
    let before = tokio::fs::read_to_string(&active).await.unwrap();
    let selected = BuiltinSource::new(bundled, Some(cache(dir.path())))
        .read()
        .await
        .unwrap();
    assert_eq!(selected["revision"], 5);
    assert_eq!(tokio::fs::read_to_string(&active).await.unwrap(), before);
}

#[tokio::test]
async fn newer_bundled_replaces_stale_active() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(9, None)).await;
    let active = active_of(dir.path());
    write(&active, &with_revision(2, Some("old"))).await;
    BuiltinSource::new(bundled, Some(cache(dir.path())))
        .read()
        .await
        .unwrap();
    let written: Value =
        serde_json::from_str(&tokio::fs::read_to_string(&active).await.unwrap()).unwrap();
    assert_eq!(written["revision"], 9);
}

#[tokio::test]
async fn same_revision_conflict_prefers_bundled_and_repairs_active() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(4, None)).await;
    let active = active_of(dir.path());
    write(&active, &with_revision(4, Some("tampered"))).await;
    let selected = BuiltinSource::new(bundled, Some(cache(dir.path())))
        .read()
        .await
        .unwrap();
    assert_eq!(selected, with_revision(4, None));
    let written: Value =
        serde_json::from_str(&tokio::fs::read_to_string(&active).await.unwrap()).unwrap();
    assert_eq!(written, with_revision(4, None));
}

#[tokio::test]
async fn corrupt_active_is_recovered_from_bundled() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(2, None)).await;
    let active = active_of(dir.path());
    tokio::fs::create_dir_all(active.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&active, "{broken").await.unwrap();
    let selected = BuiltinSource::new(bundled, Some(cache(dir.path())))
        .read()
        .await
        .unwrap();
    assert_eq!(selected["revision"], 2);
    assert!(
        serde_json::from_str::<Value>(&tokio::fs::read_to_string(&active).await.unwrap()).is_ok()
    );
}

#[tokio::test]
async fn invalid_bundled_falls_back_to_valid_active_and_both_invalid_errors() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    tokio::fs::write(&bundled, "{nope").await.unwrap();
    let active = active_of(dir.path());
    write(&active, &with_revision(6, None)).await;
    let source = BuiltinSource::new(bundled.clone(), Some(cache(dir.path())));
    assert_eq!(source.read().await.unwrap()["revision"], 6);
    tokio::fs::write(&active, "{nope").await.unwrap();
    let error = source.read().await.unwrap_err().to_string();
    assert!(
        error.contains("Bundled 与 Active ESCode Built-in Release 均不可用"),
        "{error}"
    );
}

#[tokio::test]
async fn unusable_cache_dir_degrades_to_bundled() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(1, None)).await;
    // cache root 是一个普通文件：建目录失败，必须绕过 Active 而不是失败。
    let blocker = dir.path().join("blocker");
    tokio::fs::write(&blocker, "x").await.unwrap();
    let selected = BuiltinSource::new(bundled, Some(cache(&blocker)))
        .read()
        .await
        .unwrap();
    assert_eq!(selected["revision"], 1);
}

#[tokio::test]
async fn no_cache_reads_bundled_only() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(1, None)).await;
    let source = BuiltinSource::new(bundled, None);
    assert_eq!(source.watched_files().await.len(), 1);
    assert_eq!(source.read().await.unwrap()["revision"], 1);
}

#[test]
fn provider_files_enable_the_cache_with_root_and_version_and_remote_only_when_declared() {
    use super::ProviderFiles;
    fn files(pairs: &[(&str, &str)]) -> Option<ProviderFiles> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        ProviderFiles::from_env(
            |name| map.get(name).map(String::as_str),
            Some(std::path::Path::new("/home")),
        )
    }
    let b = ("ESCODE_BUILTIN_PROVIDER_CONFIG_FILE", "/b.json");
    let p = ("ESCODE_PERSONAL_PROVIDER_CONFIG_FILE", "/p.json");
    let root = ("ESCODE_BUILTIN_CACHE_ROOT", "/cfg");
    let version = ("ESCODE_APP_VERSION", "1.2.3");
    let base = ("ESCODE_ENDPOINT_ORIGIN", "https://api.example.com");
    let remote = ("ESCODE_BUILTIN_REMOTE_REFRESH", "1");
    assert!(files(&[]).is_none());
    assert!(files(&[b]).is_none());
    assert!(files(&[b, p]).unwrap().cache.is_none());
    assert!(files(&[b, p, root]).unwrap().cache.is_none());
    // env 基址可缺省（Endpoint 可以只来自设置覆盖/默认值）。
    let cache = files(&[b, p, root, version]).unwrap().cache.unwrap();
    assert_eq!(cache.app_version, "1.2.3");
    assert_eq!(cache.endpoint.base_origin, None);
    assert_eq!(
        cache.endpoint.settings_home.as_deref(),
        Some(std::path::Path::new("/home"))
    );
    let with_base = files(&[b, p, root, version, base]).unwrap().cache.unwrap();
    assert_eq!(
        with_base.endpoint.base_origin.as_deref(),
        Some("https://api.example.com")
    );
    // 远端刷新只在 Main 显式声明且缓存启用时开启。
    assert!(!files(&[b, p, root, version]).unwrap().remote_refresh);
    assert!(
        files(&[b, p, root, version, remote])
            .unwrap()
            .remote_refresh
    );
    assert!(!files(&[b, p, remote]).unwrap().remote_refresh);
}

#[tokio::test]
async fn service_picks_up_a_newer_active_release_written_externally() {
    use super::ProviderService;
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("bundled.json");
    write(&bundled, &with_revision(1, None)).await;
    let personal = dir.path().join("personal.json");
    let service = ProviderService::new(bundled, personal).with_active_cache(cache(dir.path()));
    let before = service.model_selection_view(None).await.unwrap()["revision"]
        .as_u64()
        .unwrap();
    let active = active_of(dir.path());
    write(&active, &with_revision(2, Some("remote"))).await;
    let after = service.model_selection_view(None).await.unwrap()["revision"]
        .as_u64()
        .unwrap();
    assert!(after > before, "{after} > {before}");
}
