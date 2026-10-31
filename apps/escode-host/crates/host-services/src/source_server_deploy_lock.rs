//! 远端部署锁（S4-c，docs/specs/source-server-remote-rust-host.md §11.3）。
//!
//! 与 TS `remoteDeployLock.ts` 对齐：owner token + heartbeat + stale 600s 抢占，远端目录
//! `~/.escode/server/.deploy.lock`。锁 holder 是一条长驻 transport 命令，stdin 只用于释放标记；
//! Host 是唯一写入方，但锁目录与旧 Node 部署共用，保证混跑期间互斥。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};

use crate::source_server_exec::RemoteRunner;
use crate::source_server_transport::{quote_posix_path_arg, quote_posix_shell_arg};

pub const DEFAULT_LOCK_DIR: &str = "~/.escode/server/.deploy.lock";
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(120);
pub const DEFAULT_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
pub const LOCK_HEARTBEAT_SECONDS: u64 = 30;
pub const LOCK_STALE_SECONDS: u64 = 600;
const MAX_DIAGNOSTIC_CHARS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcquireOptions {
    pub lock_dir: String,
    pub owner_token: String,
    pub acquire_timeout: Duration,
    pub release_timeout: Duration,
}

impl Default for AcquireOptions {
    fn default() -> Self {
        Self {
            lock_dir: DEFAULT_LOCK_DIR.to_owned(),
            owner_token: uuid::Uuid::new_v4().to_string(),
            acquire_timeout: DEFAULT_ACQUIRE_TIMEOUT,
            release_timeout: DEFAULT_RELEASE_TIMEOUT,
        }
    }
}

#[derive(Debug)]
pub struct DeployLock {
    child: Child,
    stdin: Option<ChildStdin>,
    owner_token: String,
    release_timeout: Duration,
}

impl DeployLock {
    pub fn owner_token(&self) -> &str {
        &self.owner_token
    }

    /// 写入 release marker 并等待 holder 退出；超时只终止本机 holder，不删除远端锁。
    pub async fn release(mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            let marker = release_marker(&self.owner_token);
            stdin
                .write_all(format!("{marker}\n").as_bytes())
                .await
                .context("write deploy lock release marker")?;
            stdin.shutdown().await.ok();
        }
        match tokio::time::timeout(self.release_timeout, self.child.wait()).await {
            Ok(status) => {
                status.context("wait for deploy lock holder")?;
                Ok(())
            }
            Err(_) => {
                let _ = self.child.start_kill();
                let _ = self.child.wait().await;
                bail!(
                    "deploy lock release timed out after {:?} (owner={})",
                    self.release_timeout,
                    self.owner_token
                )
            }
        }
    }
}

/// 获取锁；失败/超时会终止 waiter，但绝不删除其他 owner 的锁目录。
pub async fn acquire(runner: &dyn RemoteRunner, options: &AcquireOptions) -> Result<DeployLock> {
    let script = build_lock_script(&options.lock_dir, &options.owner_token);
    let mut child = runner.spawn_piped(&script)?;
    let diagnostics = Arc::new(Mutex::new(String::new()));
    drain_stderr(&mut child, Arc::clone(&diagnostics));
    let Some(stdout) = child.stdout.take() else {
        bail!("deploy lock holder stdout missing");
    };
    let marker = acquired_marker(&options.owner_token);
    let wait_for_marker = async {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if line.contains(&marker) => return Ok(()),
                Ok(Some(_)) => continue,
                Ok(None) => {
                    let code = child.wait().await.ok().and_then(|status| status.code());
                    bail!(
                        "deploy lock holder exited before acquisition (code={code:?}){}",
                        diagnostic_suffix(&diagnostics)
                    )
                }
                Err(error) => bail!("read deploy lock marker: {error}"),
            }
        }
    };
    match tokio::time::timeout(options.acquire_timeout, wait_for_marker).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(error);
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            bail!(
                "deploy lock acquisition timed out after {:?} (owner={}){}",
                options.acquire_timeout,
                options.owner_token,
                diagnostic_suffix(&diagnostics)
            );
        }
    }
    let stdin = child.stdin.take();
    Ok(DeployLock {
        child,
        stdin,
        owner_token: options.owner_token.clone(),
        release_timeout: options.release_timeout,
    })
}

/// 与 Node `buildRemoteDeployLockScript` 同一状态机与 stale 判据。
pub fn build_lock_script(lock_dir: &str, owner_token: &str) -> String {
    let quoted_lock_dir = quote_posix_path_arg(lock_dir);
    let quoted_owner_token = quote_posix_shell_arg(owner_token);
    let parent = quote_posix_path_arg(&parent_dir_of(lock_dir));
    let acquired = quote_posix_shell_arg(&acquired_marker(owner_token));
    let release = quote_posix_shell_arg(&release_marker(owner_token));
    [
        "set -eu".to_owned(),
        format!("lock_dir={quoted_lock_dir}"),
        format!("owner_token={quoted_owner_token}"),
        "owner_file=\"$lock_dir/owner\"".to_owned(),
        "stale_dir=\"$lock_dir.stale-$owner_token\"".to_owned(),
        format!("mkdir -p {parent}"),
        "while ! mkdir \"$lock_dir\" 2>/dev/null; do".to_owned(),
        "  lock_mtime=$({ stat -c %Y \"$owner_file\" || stat -f %m \"$owner_file\" || stat -c %Y \"$lock_dir\" || stat -f %m \"$lock_dir\"; } 2>/dev/null || printf 0)".to_owned(),
        "  lock_now=$(date +%s)".to_owned(),
        format!(
            "  if [ \"$lock_mtime\" -gt 0 ] && [ $((lock_now - lock_mtime)) -ge {LOCK_STALE_SECONDS} ]; then"
        ),
        "    if command mv \"$lock_dir\" \"$stale_dir\" 2>/dev/null; then rm -rf \"$stale_dir\"; fi".to_owned(),
        "    continue".to_owned(),
        "  fi".to_owned(),
        "  sleep 1".to_owned(),
        "done".to_owned(),
        "printf %s \"$owner_token\" > \"$owner_file\"".to_owned(),
        "lock_heartbeat_pid=".to_owned(),
        "cleanup_deploy_lock() {".to_owned(),
        "  if [ -n \"${lock_heartbeat_pid:-}\" ]; then kill \"$lock_heartbeat_pid\" >/dev/null 2>&1 || true; wait \"$lock_heartbeat_pid\" 2>/dev/null || true; fi".to_owned(),
        "  current_owner=$(cat \"$owner_file\" 2>/dev/null || true)".to_owned(),
        "  if [ \"$current_owner\" = \"$owner_token\" ]; then rm -rf \"$lock_dir\"; fi".to_owned(),
        "}".to_owned(),
        "trap cleanup_deploy_lock EXIT HUP INT TERM".to_owned(),
        format!(
            // Bug 根因：heartbeat 子 shell 继承 SSH 通道的 stdin/stdout/stderr；release 后
            // `kill` 只结束子 shell，遗留的 `sleep` 仍持有通道 fd，SSH 客户端要等它退出才关闭
            // 连接，表现为 release 超时。把 heartbeat 的 fd 全部改到 /dev/null，使通道在
            // 主脚本退出时立即关闭（仍保留 owner 心跳语义）。
            "(while [ \"$(cat \"$owner_file\" 2>/dev/null || true)\" = \"$owner_token\" ]; do touch \"$owner_file\" 2>/dev/null || exit 0; sleep {LOCK_HEARTBEAT_SECONDS}; done) </dev/null >/dev/null 2>&1 &"
        ),
        "lock_heartbeat_pid=$!".to_owned(),
        format!("printf '%s\\n' {acquired}"),
        "release_marker=".to_owned(),
        "IFS= read -r release_marker || true".to_owned(),
        format!("if [ \"$release_marker\" != {release} ]; then echo 'invalid deploy lock release marker' >&2; exit 1; fi"),
    ]
    .join("\n")
}

fn acquired_marker(owner_token: &str) -> String {
    format!("escode-deploy-lock-acquired:{owner_token}")
}

fn release_marker(owner_token: &str) -> String {
    format!("escode-deploy-lock-release:{owner_token}")
}

fn parent_dir_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent.to_owned(),
        _ => ".".to_owned(),
    }
}

fn drain_stderr(child: &mut Child, sink: Arc<Mutex<String>>) {
    let Some(stderr) = child.stderr.take() else {
        return;
    };
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(mut text) = sink.lock() {
                text.push_str(&line);
                text.push('\n');
                if text.len() > MAX_DIAGNOSTIC_CHARS {
                    let drain = text.len() - MAX_DIAGNOSTIC_CHARS;
                    text.drain(..drain);
                }
            }
        }
    });
}

fn diagnostic_suffix(sink: &Arc<Mutex<String>>) -> String {
    let Ok(text) = sink.lock() else {
        return String::new();
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {trimmed}")
    }
}

#[cfg(test)]
#[path = "source_server_deploy_lock_tests.rs"]
mod tests;
