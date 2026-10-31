#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use async_trait::async_trait;
#[cfg(unix)]
use tokio::process::Child;

use super::build_lock_script;
#[cfg(unix)]
use super::{AcquireOptions, acquire};
#[cfg(unix)]
use crate::source_server_exec::{CommandOutput, RemoteRunner, run_command, spawn_piped_command};
#[cfg(unix)]
use crate::source_server_transport::{TransportCommand, quote_posix_path_arg};

#[cfg(unix)]
struct LocalRunner;

#[cfg(unix)]
#[async_trait]
impl RemoteRunner for LocalRunner {
    async fn run(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> anyhow::Result<CommandOutput> {
        run_command(&local_command(script), stdin, timeout).await
    }

    fn spawn_piped(&self, script: &str) -> anyhow::Result<Child> {
        spawn_piped_command(&local_command(script))
    }
}

#[cfg(unix)]
fn local_command(script: &str) -> TransportCommand {
    TransportCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: BTreeMap::new(),
    }
}

#[cfg(unix)]
fn options(lock_dir: &std::path::Path, timeout: u64) -> AcquireOptions {
    AcquireOptions {
        lock_dir: lock_dir.to_string_lossy().into_owned(),
        acquire_timeout: Duration::from_secs(timeout),
        release_timeout: Duration::from_secs(5),
        ..AcquireOptions::default()
    }
}

#[cfg(unix)]
#[tokio::test]
async fn acquires_and_releases_the_lock_directory() {
    let temp = tempfile::tempdir().expect("temp");
    let lock_dir = temp.path().join(".deploy.lock");
    let lock = acquire(&LocalRunner, &options(&lock_dir, 5))
        .await
        .expect("acquire");
    assert!(lock_dir.is_dir());
    let owner = std::fs::read_to_string(lock_dir.join("owner")).expect("owner");
    assert_eq!(owner, lock.owner_token());
    lock.release().await.expect("release");
    assert!(!lock_dir.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_second_owner_waits_for_the_first_release() {
    let temp = tempfile::tempdir().expect("temp");
    let lock_dir = temp.path().join(".deploy.lock");
    let first = acquire(&LocalRunner, &options(&lock_dir, 10))
        .await
        .expect("first");
    let second_options = options(&lock_dir, 10);
    let mut second = tokio::spawn(async move { acquire(&LocalRunner, &second_options).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut second)
            .await
            .is_err(),
        "second owner must wait while the first holds the lock"
    );

    first.release().await.expect("release first");
    let second = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("second must acquire after release")
        .expect("join")
        .expect("acquire");
    second.release().await.expect("release second");
}

#[cfg(unix)]
#[tokio::test]
async fn stale_lock_is_reclaimed() {
    let temp = tempfile::tempdir().expect("temp");
    let lock_dir = temp.path().join(".deploy.lock");
    std::fs::create_dir_all(&lock_dir).expect("lock dir");
    std::fs::write(lock_dir.join("owner"), "foreign").expect("owner");
    let quoted = quote_posix_path_arg(&lock_dir.to_string_lossy());
    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!("touch -t 200001010000 {quoted}/owner {quoted}"))
        .status()
        .await
        .expect("touch");
    assert!(status.success());

    let lock = tokio::time::timeout(
        Duration::from_secs(5),
        acquire(&LocalRunner, &options(&lock_dir, 5)),
    )
    .await
    .expect("stale takeover must not block")
    .expect("acquire");
    lock.release().await.expect("release");
}

#[cfg(unix)]
#[tokio::test]
async fn timed_out_waiter_leaves_the_foreign_lock_untouched() {
    let temp = tempfile::tempdir().expect("temp");
    let lock_dir = temp.path().join(".deploy.lock");
    std::fs::create_dir_all(&lock_dir).expect("lock dir");
    std::fs::write(lock_dir.join("owner"), "foreign").expect("owner");
    let error = acquire(&LocalRunner, &options(&lock_dir, 1))
        .await
        .expect_err("timeout");
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(lock_dir.is_dir(), "foreign lock must survive");
    assert_eq!(
        std::fs::read_to_string(lock_dir.join("owner")).expect("owner"),
        "foreign"
    );
}

#[test]
fn lock_script_expands_home_and_quotes_tokens() {
    let script = build_lock_script("~/.escode/server/.deploy.lock", "owner-1");
    assert!(script.contains("lock_dir=\"$HOME\"'/.escode/server/.deploy.lock'"));
    assert!(script.contains("owner_token='owner-1'"));
    assert!(script.contains("escode-deploy-lock-acquired:owner-1"));
    assert!(script.contains("escode-deploy-lock-release:owner-1"));
    assert!(script.contains("mkdir -p \"$HOME\"'/.escode/server'"));
}
