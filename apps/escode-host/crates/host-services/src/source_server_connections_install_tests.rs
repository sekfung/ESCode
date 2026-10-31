use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use async_trait::async_trait;
use serde_json::json;
use tokio::process::Child;

use super::test_support::*;
use super::*;
#[cfg(unix)]
use crate::source_server_connections::ConnectIntent;
use crate::source_server_connections::{ConnectionsOptions, SourceServerConnections};
use crate::source_server_exec::{CommandOutput, RemoteRunner, ResolvedTransport};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn explicit_connect_installs_missing_release_then_goes_online() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let platform = platform_arch();
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform);
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home.clone()));
    let connections = connections(
        Arc::clone(&registry),
        runner,
        Some(RemoteAssetsConfig {
            release_dir: Some(release_dir),
            cdn_base_urls: Vec::new(),
            app_version: "3.15.2".to_owned(),
        }),
    );
    let mut events = registry.subscribe();
    connections.connect(PROFILE).await.expect("connect");
    let mut states = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let crate::source_server::SourceServerEvent::State(value) = event {
            states.push(value);
        }
    }
    let names: Vec<&str> = states
        .iter()
        .filter_map(|value| value["state"]["state"].as_str())
        .collect();
    assert!(names.contains(&"needs-install"), "states: {names:?}");
    assert!(names.contains(&"bootstrapping"));
    assert_eq!(names.last(), Some(&"online"));
    let install_event = states
        .iter()
        .find(|value| value["state"]["state"] == json!("needs-install"))
        .expect("needs-install state");
    assert!(
        install_event["state"]["installPreview"]["sizeBytes"]
            .as_u64()
            .is_some_and(|size| size > 0)
    );
    assert_eq!(
        install_event["state"]["installPreview"]["version"],
        json!("3.15.2")
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".escode/server/node")).expect("node"),
        "node"
    );
    assert_eq!(connections.connected_count(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn recovery_connect_never_installs() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let platform = platform_arch();
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform);
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home.clone()));
    let connections = connections(
        Arc::clone(&registry),
        Arc::clone(&runner),
        Some(RemoteAssetsConfig {
            release_dir: Some(release_dir),
            cdn_base_urls: Vec::new(),
            app_version: "3.15.2".to_owned(),
        }),
    );
    connections
        .connect_with(PROFILE, ConnectIntent::Recover)
        .await
        .expect("recover connect");
    assert_eq!(runner.calls(), 0, "recovery must not probe or install");
    assert!(!home.join(".escode/server/node").exists());
    assert_eq!(connections.connected_count(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn install_preparation_failure_is_explicit_and_never_online() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections(
        Arc::clone(&registry),
        runner,
        Some(RemoteAssetsConfig {
            release_dir: None,
            cdn_base_urls: Vec::new(),
            app_version: "3.15.2".to_owned(),
        }),
    );
    let error = connections.connect(PROFILE).await.expect_err("must fail");
    assert!(format!("{error:#}").contains("remote-install-failed"));
    let state = registry.snapshot().expect("snapshot")["states"][0].clone();
    assert_eq!(state["state"], json!("failed"));
    assert_eq!(state["errorCode"], json!("remote-install-failed"));
    assert_eq!(connections.connected_count(), 0);
}

/// S4-h：preflight 发现 host key 变化时必须显式失败，且不能被标成可自动重试。
#[tokio::test(flavor = "multi_thread")]
async fn host_key_change_in_preflight_is_not_retryable() {
    struct HostKeyChangedRunner;

    #[async_trait]
    impl RemoteRunner for HostKeyChangedRunner {
        async fn run(
            &self,
            _script: &str,
            _stdin: Option<&[u8]>,
            _timeout: Duration,
        ) -> anyhow::Result<CommandOutput> {
            bail!(
                "remote command failed (exit 255): WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed."
            );
        }

        fn spawn_piped(&self, _script: &str) -> anyhow::Result<Child> {
            bail!("host key changed")
        }
    }

    let registry = registry();
    let options = ConnectionsOptions::new("3.15.2", |_| {
        Box::pin(async {
            Ok(ResolvedTransport {
                command: fixture_command(),
                runner: Arc::new(HostKeyChangedRunner),
            })
        })
    })
    .with_install(Some(InstallOptions {
        assets: RemoteAssetsConfig {
            release_dir: Some(std::path::PathBuf::from("/nonexistent")),
            cdn_base_urls: Vec::new(),
            app_version: "3.15.2".to_owned(),
        },
        lock: AcquireOptions::default(),
    }));
    let connections = SourceServerConnections::new(Arc::clone(&registry), options);
    let error = connections.connect(PROFILE).await.expect_err("must fail");
    assert!(
        format!("{error:#}").contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
        "{error:#}"
    );
    let state = registry.snapshot().expect("snapshot")["states"][0].clone();
    assert_eq!(state["state"], json!("failed"));
    assert_eq!(state["errorCode"], json!("remote-host-key-changed"));
    assert_eq!(state["retryable"], json!(false));
    assert_eq!(connections.connected_count(), 0);
}
