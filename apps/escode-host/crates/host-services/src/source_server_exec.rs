//! Source Server 一次性远端脚本执行（S4-c，docs/specs/source-server-remote-rust-host.md §11.3）。
//!
//! 安装、preflight 与部署锁都经同一 transport 执行 POSIX 脚本：调用方只提供脚本与可选 stdin，
//! SSH/WSL/Docker 差异由 [`TransportCommand`] 收口。`RemoteExecutor` 不读进程 env、不缓存凭据。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::source_server_transport::{
    BuildOptions, TransportCommand, TransportSecrets, TransportTarget, build_transport_command,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// 脚本执行面；生产用 [`RemoteExecutor`]，测试可注入本地 `sh`。
#[async_trait]
pub trait RemoteRunner: Send + Sync {
    async fn run(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<CommandOutput>;

    /// 长驻脚本（部署锁）：stdin/stdout 都保持可读写，由调用方决定何时释放。
    fn spawn_piped(&self, script: &str) -> Result<Child>;
}

/// 一次 transport 解析的产物：启动 server 的命令 + 同一链路上的脚本执行面。
/// 安装/preflight 必须复用同一次凭据解密结果，不能二次解析出另一个目标。
#[derive(Clone)]
pub struct ResolvedTransport {
    pub command: TransportCommand,
    pub runner: Arc<dyn RemoteRunner>,
}

impl ResolvedTransport {
    /// 只启动 server、不执行安装脚本的调用方（测试/未配置安装）。
    pub fn command_only(command: TransportCommand) -> Self {
        struct UnavailableRunner;

        #[async_trait::async_trait]
        impl RemoteRunner for UnavailableRunner {
            async fn run(
                &self,
                _script: &str,
                _stdin: Option<&[u8]>,
                _timeout: Duration,
            ) -> Result<CommandOutput> {
                bail!("this transport has no script runner")
            }

            fn spawn_piped(&self, _script: &str) -> Result<Child> {
                bail!("this transport has no script runner")
            }
        }

        Self {
            command,
            runner: Arc::new(UnavailableRunner),
        }
    }
}

/// transport 目标 + 已解出的凭据；只在一次连接准备期间存活。
#[derive(Clone, Debug)]
pub struct RemoteExecutor {
    target: TransportTarget,
    secrets: TransportSecrets,
    askpass_program: PathBuf,
    known_hosts: Option<PathBuf>,
}

impl RemoteExecutor {
    pub fn new(
        target: TransportTarget,
        secrets: TransportSecrets,
        askpass_program: impl Into<PathBuf>,
    ) -> Self {
        Self {
            target,
            secrets,
            askpass_program: askpass_program.into(),
            known_hosts: None,
        }
    }

    /// SSH 的 Desktop 自有 known_hosts（spec §11.8）；WSL/Docker 不需要。
    pub fn with_known_hosts(mut self, known_hosts: Option<PathBuf>) -> Self {
        self.known_hosts = known_hosts;
        self
    }

    pub fn build_command(&self, script: &str) -> Result<TransportCommand> {
        let remote_command = vec!["sh".to_owned(), "-lc".to_owned(), script.to_owned()];
        build_transport_command(
            &self.target,
            &self.secrets,
            &BuildOptions {
                askpass_program: &self.askpass_program,
                remote_command: &remote_command,
                known_hosts: self.known_hosts.as_deref(),
            },
        )
    }
}

#[async_trait]
impl RemoteRunner for RemoteExecutor {
    async fn run(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<CommandOutput> {
        let command = self.build_command(script)?;
        run_command(&command, stdin, timeout).await
    }

    fn spawn_piped(&self, script: &str) -> Result<Child> {
        let command = self.build_command(script)?;
        spawn_piped_command(&command)
    }
}

/// 与 `RemoteExecutor::run` 同语义，但直接接受已构造的 transport 命令（测试/诊断复用）。
pub async fn run_command(
    command: &TransportCommand,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut child = base_command(command)
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .spawn()
        .with_context(|| {
            format!(
                "cannot start remote transport {}",
                command.program.display()
            )
        })?;
    escode_host_control::logging::info(
        "source-server",
        format!(
            "spawned remote command transport {:?}: pid={:?}",
            command.program,
            child.id()
        ),
    );
    let mut stdin_writer = child.stdin.take();
    let write = async move {
        if let Some(bytes) = stdin {
            let mut writer = stdin_writer
                .take()
                .context("remote transport stdin missing")?;
            writer
                .write_all(bytes)
                .await
                .context("write remote stdin")?;
            writer.shutdown().await.context("close remote stdin")?;
        }
        Ok::<(), anyhow::Error>(())
    };
    let wait = child.wait_with_output();
    let (write_result, output) = tokio::time::timeout(timeout, async { tokio::join!(write, wait) })
        .await
        .map_err(|_| anyhow::anyhow!("remote command timed out after {timeout:?}"))?;
    write_result?;
    let output = output.context("wait for remote command")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_owned(), |code| code.to_string());
        bail!("remote command failed (exit {code}): {}", stderr.trim(),);
    }
    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code: output.status.code(),
    })
}

pub fn spawn_piped_command(command: &TransportCommand) -> Result<Child> {
    base_command(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "cannot start remote transport {}",
                command.program.display()
            )
        })
}

fn base_command(command: &TransportCommand) -> Command {
    let mut child = Command::new(&command.program);
    child
        .args(&command.args)
        .envs(&command.env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    child
}

/// 读取子进程输出直到 EOF（部署锁用于等待 acquired marker）。
pub async fn read_stdout_line(child: &mut Child, timeout: Duration) -> Result<Option<String>> {
    let Some(stdout) = child.stdout.take() else {
        bail!("remote command stdout missing");
    };
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    let read = tokio::time::timeout(timeout, reader.read_line(&mut line))
        .await
        .map_err(|_| anyhow::anyhow!("remote command produced no output within {timeout:?}"))??;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

/// 兼容旧调用点：脚本只经 transport 执行，不读取 Host 进程 env。
pub async fn run_script(
    target: &TransportTarget,
    secrets: &TransportSecrets,
    askpass_program: &Path,
    known_hosts: Option<&Path>,
    script: &str,
    timeout: Duration,
) -> Result<CommandOutput> {
    RemoteExecutor::new(target.clone(), secrets.clone(), askpass_program)
        .with_known_hosts(known_hosts.map(Path::to_path_buf))
        .run(script, None, timeout)
        .await
}

#[cfg(test)]
#[path = "source_server_exec_tests.rs"]
mod tests;
