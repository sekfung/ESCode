// 更新流程依赖本机 `sh` fixture 模拟远端安装脚本；Windows 主链由桌面 E2E 覆盖（S4-i）。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::test_support::*;
use super::*;
use crate::source_server_connections::{
    ConnectIntent, ConnectionsOptions, SourceServerConnections,
};
use crate::source_server_deploy_lock::AcquireOptions;
use crate::source_server_exec::{RemoteLink, ResolvedTransport};
use crate::source_server_transport::{TransportCommand, quote_posix_shell_arg};
use crate::source_server_update::UpdateRequest;

const WORKSPACE: &str = "/remote/workspace";
const WORKSPACE_IDENTITY: &str = "remote:/remote/workspace";

fn assets(release_dir: PathBuf) -> Option<RemoteAssetsConfig> {
    Some(RemoteAssetsConfig {
        release_dir: Some(release_dir),
        cdn_base_urls: Vec::new(),
        app_version: "3.15.2".to_owned(),
    })
}

fn connections_with_commands(
    registry: Arc<crate::source_server::SourceServerRegistry>,
    runner: Arc<LocalRunner>,
    assets: Option<RemoteAssetsConfig>,
    commands: Vec<TransportCommand>,
    compatibility: crate::source_server_compatibility::CompatibilityManifest,
) -> SourceServerConnections {
    let commands = Arc::new(commands);
    let cursor = Arc::new(AtomicUsize::new(0));
    let options = ConnectionsOptions::new("3.15.2", move |_profile| {
        let commands = Arc::clone(&commands);
        let cursor = Arc::clone(&cursor);
        let runner = Arc::clone(&runner);
        Box::pin(async move {
            let index = cursor.fetch_add(1, Ordering::Relaxed);
            let command = commands[index.min(commands.len().saturating_sub(1))].clone();
            Ok(ResolvedTransport {
                link: RemoteLink::Process(command),
                runner,
            })
        })
    })
    .with_compatibility(compatibility)
    .with_install(
        assets.map(|assets| crate::source_server_bootstrap::InstallOptions {
            assets,
            lock: AcquireOptions {
                acquire_timeout: Duration::from_secs(10),
                release_timeout: Duration::from_secs(5),
                ..AcquireOptions::default()
            },
        }),
    );
    SourceServerConnections::new(registry, options)
}

fn request(force: bool) -> UpdateRequest {
    UpdateRequest {
        profile_id: PROFILE.to_owned(),
        force,
        expected_target: None,
    }
}

fn state(registry: &crate::source_server::SourceServerRegistry) -> Value {
    registry.snapshot().expect("snapshot")["states"][0].clone()
}

fn logs(registry: &crate::source_server::SourceServerRegistry) -> Vec<Value> {
    registry.recent_operation_logs(PROFILE)
}

fn fixture_with_version(version: &str) -> TransportCommand {
    let compatibility = json!({
        "schemaVersion": 1,
        "productVersion": version,
        "minCompatibleVersion": "3.0.0",
    })
    .to_string();
    fixture_command_with_env(&[
        ("ESCODE_FIXTURE_VERSION", version),
        ("ESCODE_FIXTURE_COMPATIBILITY", compatibility.as_str()),
    ])
}

/// 命令只在远端 release 的 server bundle 不是 `bad` 时启动 fixture；
/// 用于验证更新安装后新 server 失败时回滚旧 release 并恢复旧连接。
fn guarded_fixture_command(home: &Path) -> TransportCommand {
    let fixture = fixture_command();
    let fixture_path = fixture
        .args
        .get(2)
        .map(|value| value.to_string_lossy().into_owned())
        .expect("fixture path");
    let script = format!(
        "set -eu\nif [ \"$(cat \"$HOME/.escode/server/zcode-server.cjs\" 2>/dev/null || true)\" = bad ]; then echo 'broken release' >&2; exit 1; fi\nexec node --import tsx {}",
        quote_posix_shell_arg(&fixture_path)
    );
    let mut env = fixture.env;
    env.insert("HOME".to_owned(), home.to_string_lossy().into_owned());
    TransportCommand {
        program: PathBuf::from("sh"),
        args: vec!["-c".into(), script.into()],
        env,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn running_tasks_block_update_until_force() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform_arch());
    let registry = registry_with_declared(&[(WORKSPACE, WORKSPACE_IDENTITY)]);
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        runner,
        assets(release_dir),
        vec![fixture_command_with_env(&[(
            "ESCODE_FIXTURE_TASK_STATUS",
            "running",
        )])],
        crate::source_server_compatibility::CompatibilityManifest::new("3.15.2", "3.0.0"),
    );
    connections.connect(PROFILE).await.expect("connect");
    assert_eq!(state(&registry)["state"], json!("online"));

    let error = connections
        .update_source(request(false))
        .await
        .expect_err("running tasks must block update");
    assert_eq!(error.code, Some("source-server-update-running-tasks"));
    let blocked = state(&registry);
    assert_eq!(blocked["state"], json!("online"));
    assert_eq!(
        blocked["runningTasks"][0]["sessionId"],
        json!("task-remote-1")
    );
    assert_eq!(
        blocked["runningTasks"][0]["workspacePath"],
        json!(WORKSPACE)
    );
    assert!(connections.client(PROFILE).is_some());
    assert_eq!(connections.connected_count(), 1);

    connections
        .update_source(request(true))
        .await
        .expect("forced update");
    let online = state(&registry);
    assert_eq!(online["state"], json!("online"));
    assert!(online.get("runningTasks").is_none());
    assert_eq!(connections.connected_count(), 2);
    assert!(
        logs(&registry)
            .iter()
            .any(|log| log["message"] == json!("source server updated to 3.15.2"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_rejects_changed_expected_target_without_transport() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        Arc::clone(&runner),
        None,
        vec![fixture_command()],
        crate::source_server_compatibility::CompatibilityManifest::local("3.15.2"),
    );
    let error = connections
        .update_source(UpdateRequest {
            profile_id: PROFILE.to_owned(),
            force: false,
            expected_target: Some(json!({
                "kind": "ssh",
                "host": "other.example",
                "port": 22,
                "username": "dev",
            })),
        })
        .await
        .expect_err("target mismatch");
    assert_eq!(error.code, Some("remote-target-changed"));
    assert_eq!(runner.calls(), 0);
    assert_eq!(state(&registry)["state"], json!("absent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_task_list_requires_force() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    write(&home.join(".escode/server/node"), "node", true);
    write(
        &home.join(".escode/server/zcode-server.cjs"),
        "server",
        false,
    );
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform_arch());
    let registry = registry_with_declared(&[(WORKSPACE, WORKSPACE_IDENTITY)]);
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        runner,
        assets(release_dir),
        vec![fixture_command_with_env(&[(
            "ESCODE_FIXTURE_NO_TASK_CHANNEL",
            "1",
        )])],
        crate::source_server_compatibility::CompatibilityManifest::new("3.15.2", "3.0.0"),
    );

    let error = connections
        .update_source(request(false))
        .await
        .expect_err("unavailable list must require force");
    assert_eq!(error.code, Some("source-server-update-running-tasks"));
    assert!(error.message.contains("unavailable"));
    assert_eq!(state(&registry)["state"], json!("absent"));

    connections
        .update_source(request(true))
        .await
        .expect("forced update");
    assert_eq!(state(&registry)["state"], json!("online"));
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_failure_keeps_online_connection() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform_arch());
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        runner,
        assets(release_dir.clone()),
        vec![fixture_command()],
        crate::source_server_compatibility::CompatibilityManifest::local("3.15.2"),
    );
    connections.connect(PROFILE).await.expect("connect");
    std::fs::remove_file(release_dir.join("server/zcode-server.cjs")).expect("remove artifact");

    let error = connections
        .update_source(request(false))
        .await
        .expect_err("missing release must fail");
    assert!(error.message.contains("remote-install-failed"));
    let failed = state(&registry);
    assert_eq!(failed["state"], json!("online"));
    assert_eq!(failed["errorCode"], json!("remote-install-failed"));
    assert!(connections.client(PROFILE).is_some());
    assert_eq!(connections.connected_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn start_failure_rolls_back_release_and_restores_connection() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform_arch());
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home.clone()));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        runner,
        assets(release_dir.clone()),
        vec![guarded_fixture_command(&home)],
        crate::source_server_compatibility::CompatibilityManifest::local("3.15.2"),
    );
    connections.connect(PROFILE).await.expect("connect");
    assert_eq!(state(&registry)["state"], json!("online"));
    // 更新 release 的主 bundle 变成坏版本：安装后新 server 无法启动。
    write(&release_dir.join("server/zcode-server.cjs"), "bad", false);

    let error = connections
        .update_source(request(true))
        .await
        .expect_err("broken release must fail");
    assert!(error.message.contains("broken release") || error.message.contains("escode-hello"));
    let restored = std::fs::read_to_string(home.join(".escode/server/zcode-server.cjs"))
        .expect("restored server");
    assert_eq!(restored, "server");
    let online = state(&registry);
    assert_eq!(online["state"], json!("online"));
    assert!(online["error"].as_str().is_some());
    assert!(connections.client(PROFILE).is_some());
    assert!(
        logs(&registry)
            .iter()
            .any(|log| log["phase"] == json!("failed"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn successful_update_refreshes_remote_version() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform_arch());
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        runner,
        assets(release_dir),
        vec![
            fixture_with_version("3.14.0"),
            fixture_with_version("3.15.2"),
            fixture_with_version("3.15.2"),
        ],
        crate::source_server_compatibility::CompatibilityManifest::new("3.15.2", "3.0.0"),
    );
    connections.connect(PROFILE).await.expect("connect");
    let old = state(&registry);
    assert_eq!(old["currentVersion"], json!("3.14.0"));
    assert_eq!(old["updateAvailability"], json!("available"));
    let old_client = connections.client(PROFILE).expect("old client");

    connections
        .update_source(request(false))
        .await
        .expect("update");
    // 旧句柄必须释放，新的 attachment 才能拿到换代后的远端 scope。
    tokio::time::timeout(Duration::from_secs(1), old_client.wait_closed())
        .await
        .expect("old client closes on update");
    let new_client = connections.client(PROFILE).expect("new client");
    assert!(!Arc::ptr_eq(&old_client, &new_client));
    let online = state(&registry);
    assert_eq!(online["state"], json!("online"));
    assert_eq!(online["currentVersion"], json!("3.15.2"));
    assert_eq!(online["updateAvailability"], json!("none"));
    assert_eq!(connections.connected_count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_intent_never_installs_or_updates() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release_dir = temp.path().join("release");
    write_release(&release_dir, platform_arch());
    let registry = registry();
    let runner = Arc::new(LocalRunner::new(home));
    let connections = connections_with_commands(
        Arc::clone(&registry),
        runner.clone(),
        assets(release_dir),
        vec![fixture_command()],
        crate::source_server_compatibility::CompatibilityManifest::local("3.15.2"),
    );
    connections
        .connect_with(PROFILE, ConnectIntent::Recover)
        .await
        .expect("recover");
    assert_eq!(runner.calls(), 0);
    assert_eq!(state(&registry)["state"], json!("online"));
}
