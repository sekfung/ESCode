use super::*;

#[test]
fn production_ignores_the_override_and_falls_back_to_the_default() {
    assert_eq!(
        resolve_origin(true, None, Some("https://override.example.com")).unwrap(),
        "https://zcode.z.ai"
    );
    assert_eq!(
        resolve_origin(
            true,
            Some(" https://Base.example.com:443/x "),
            Some("https://o.example.com")
        )
        .unwrap(),
        "https://base.example.com"
    );
}

#[test]
fn non_production_prefers_the_override_then_the_base_then_the_default() {
    assert_eq!(
        resolve_origin(
            false,
            Some("https://base.example.com"),
            Some("http://localhost:8080/p")
        )
        .unwrap(),
        "http://localhost:8080"
    );
    assert_eq!(
        resolve_origin(false, Some("https://base.example.com"), Some("   ")).unwrap(),
        "https://base.example.com"
    );
    assert_eq!(
        resolve_origin(false, None, None).unwrap(),
        "https://zcode.z.ai"
    );
}

#[test]
fn invalid_origins_fail_the_resolution() {
    assert!(resolve_origin(false, None, Some("ftp://x.example.com")).is_err());
    assert!(resolve_origin(true, Some("not a url"), None).is_err());
}

#[tokio::test]
async fn the_source_reads_the_override_live_from_the_settings_file() {
    let home = tempfile::tempdir().unwrap();
    let source = EndpointSource {
        escode_env: Some("test".into()),
        base_origin: Some("https://base.example.com".into()),
        settings_home: Some(home.path().to_path_buf()),
    };
    assert_eq!(source.resolve().await.unwrap(), "https://base.example.com");
    let path = crate::settings::settings_path(home.path());
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &path,
        r#"{"escodeEndpointOrigin":"https://switched.example.com/"}"#,
    )
    .await
    .unwrap();
    assert_eq!(
        source.resolve().await.unwrap(),
        "https://switched.example.com"
    );
    // 生产环境不读覆盖。
    let production = EndpointSource {
        escode_env: Some("production".into()),
        ..source.clone()
    };
    assert_eq!(
        production.resolve().await.unwrap(),
        "https://base.example.com"
    );
    // 坏文件 = 无覆盖，且不隔离/不改写。
    tokio::fs::write(&path, "{broken").await.unwrap();
    assert_eq!(source.resolve().await.unwrap(), "https://base.example.com");
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "{broken");
}
