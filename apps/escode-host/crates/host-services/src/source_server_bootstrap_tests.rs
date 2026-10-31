use super::test_support::*;
use super::*;

#[tokio::test]
async fn prepares_local_upload_only_for_missing_components() {
    let temp = tempfile::tempdir().expect("temp");
    let release_dir = temp.path().join("release");
    let platform = platform_arch();
    write_release(&release_dir, platform);
    let runner = FakeRunner {
        stdout: format!(
            "{}@@node-runtime exists=1\n@@server-bundle exists=1\n@@node-pty exists=0\n@@glm exists=0\n@@bfs exists=0\n@@ripgrep exists=0\n@@ugrep exists=0\n",
            probe_stdout(true)
        ),
    };
    let assets = RemoteAssetsConfig {
        release_dir: Some(release_dir),
        cdn_base_urls: vec!["https://cdn.example/remote".to_owned()],
        app_version: "3.15.2".to_owned(),
    };
    let prepared = prepare_install(&runner, &assets)
        .await
        .expect("prepare")
        .expect("install needed");
    assert!(matches!(prepared.mode, InstallMode::LocalUpload { .. }));
    assert_eq!(prepared.plan.components.len(), 5);
    assert!(prepared.plan.components.iter().all(|component| {
        !matches!(
            component.component.id.as_str(),
            "node-runtime" | "server-bundle"
        )
    }));
    let preview = prepared.preview.expect("preview");
    assert!(preview.size_bytes > 0);
    assert_eq!(preview.version, "3.15.2");
}

#[tokio::test]
async fn falls_back_to_remote_download_when_local_artifacts_are_incomplete() {
    let temp = tempfile::tempdir().expect("temp");
    let platform = platform_arch();
    let release_dir = temp.path().join("release");
    write_manifest(&release_dir, platform);
    let runner = FakeRunner {
        stdout: probe_stdout(true),
    };
    let assets = RemoteAssetsConfig {
        release_dir: Some(release_dir),
        cdn_base_urls: vec!["https://cdn.example/remote".to_owned()],
        app_version: "3.15.2".to_owned(),
    };
    let prepared = prepare_install(&runner, &assets)
        .await
        .expect("prepare")
        .expect("install needed");
    assert!(matches!(prepared.mode, InstallMode::RemoteDownload { .. }));
    // 本地没有 artifact，无法给出真实大小：省略预览而不是编造正数。
    assert!(prepared.preview.is_none());
}

#[tokio::test]
async fn skips_install_when_every_component_exists() {
    let temp = tempfile::tempdir().expect("temp");
    let platform = platform_arch();
    let release_dir = temp.path().join("release");
    write_manifest(&release_dir, platform);
    let runner = FakeRunner {
        stdout: format!(
            "{}@@node-runtime exists=1\n@@server-bundle exists=1\n@@node-pty exists=1\n@@glm exists=1\n@@bfs exists=1\n@@ripgrep exists=1\n@@ugrep exists=1\n",
            probe_stdout(true)
        ),
    };
    let assets = RemoteAssetsConfig {
        release_dir: Some(release_dir),
        cdn_base_urls: Vec::new(),
        app_version: "3.15.2".to_owned(),
    };
    assert!(
        prepare_install(&runner, &assets)
            .await
            .expect("prepare")
            .is_none()
    );
}

#[test]
fn classifies_install_failures() {
    assert_eq!(
        classify_install_failure(&anyhow!("No space left on device")),
        "remote-space-insufficient"
    );
    assert_eq!(
        classify_install_failure(&anyhow!("mv: Permission denied")),
        "remote-permission-denied"
    );
    assert_eq!(
        classify_install_failure(&anyhow!("remote artifact download or checksum failed")),
        "remote-download-failed"
    );
    assert_eq!(
        classify_install_failure(&anyhow!("tar: unexpected EOF")),
        "remote-install-failed"
    );
}

/// 主机 key 变化优先于安装阶段错误码，避免 UI 把安全事件当成可重试的安装失败。
#[test]
fn host_key_change_wins_over_install_error_codes() {
    assert_eq!(
        install_error_code(&anyhow!(
            "remote-install-failed: WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!"
        )),
        Some("remote-host-key-changed")
    );
    assert_eq!(
        install_error_code(&anyhow!(
            "remote-preflight-failed: Host key verification failed."
        )),
        Some("remote-host-key-changed")
    );
    assert_eq!(
        install_error_code(&anyhow!("remote-preflight-failed: curl missing")),
        Some("remote-preflight-failed")
    );
    assert_eq!(install_error_code(&anyhow!("tar: unexpected EOF")), None);
}
