//! runtime-link：Host 认识 runtime 的唯一边界（rust-u2-host-skeleton.md §3 / §5 第 9 项）。
//!
//! 两种链路对上层同形（`send_line` / `line`）：
//! - `Embedded`：同进程 `escode_cli_rust::embed::spawn`（阶段 1 库化产物，默认）；
//! - `Child`：`app-server --stdio` 子进程（差分、回退与故障隔离选项）。
//!
//! 链路上的帧面与 `app-server --stdio` 完全一致（NDJSON），所以链路之上代码不区分传输。

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result};
use escode_cli_rust::embed::{self, RuntimeOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

/// 选择器：默认 embedded；`child` 需要 CLI 可执行文件路径。
#[derive(Clone, Debug)]
pub enum Link {
    Embedded,
    Child { binary: PathBuf },
}

/// 一条已打开的 runtime 链路。读端由帧泵独占，写端可并发（读不阻塞写）。
pub struct RuntimeLink {
    pub reader: RuntimeReader,
    pub writer: RuntimeWriter,
    pub lifecycle: Lifecycle,
}

pub enum RuntimeReader {
    Embedded(escode_cli_app_server::EmbeddedIoReader),
    Child(BufReader<tokio::process::ChildStdout>),
}

pub enum RuntimeWriter {
    Embedded(escode_cli_app_server::EmbeddedIoWriter),
    Child(tokio::process::ChildStdin),
}

/// 链路的生命周期句柄（存活判定与收口）。
pub enum Lifecycle {
    Embedded(Box<EmbeddedRuntime>),
    Child(Box<ChildRuntime>),
}

/// embedded 链路的句柄束（实例任务、取消令牌、残余账目）。
pub struct EmbeddedRuntime {
    task: tokio::task::JoinHandle<Result<()>>,
    cancel: CancellationToken,
    tasks: std::sync::Arc<escode_cli_core_api::scope::Spawner>,
}

/// child 链路的句柄束（子进程）。
pub struct ChildRuntime {
    child: Child,
}

impl RuntimeLink {
    /// 按选择器打开链路。
    pub fn open(link: &Link, options: RuntimeOptions) -> Result<Self> {
        match link {
            Link::Embedded => {
                let embedded = embed::spawn(options);
                let (reader, writer) = embedded.io.split();
                Ok(Self {
                    reader: RuntimeReader::Embedded(reader),
                    writer: RuntimeWriter::Embedded(writer),
                    lifecycle: Lifecycle::Embedded(Box::new(EmbeddedRuntime {
                        task: embedded.task,
                        cancel: embedded.cancel,
                        tasks: embedded.tasks,
                    })),
                })
            }
            Link::Child { binary } => {
                let mut command = tokio::process::Command::new(binary);
                command
                    .arg("app-server")
                    .arg("--stdio")
                    .arg("--surface")
                    .arg("desktop")
                    .arg("--cwd")
                    .arg(&options.cwd)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .env_clear()
                    .envs(options.env.iter().map(|(k, v)| (k.clone(), v.clone())))
                    .env("HOME", &options.home)
                    .env("USERPROFILE", &options.home);
                if let Some(data_dir) = &options.data_dir {
                    command.arg("--data-dir").arg(data_dir);
                }
                if let Some(config) = &options.config {
                    command.arg("--config").arg(config);
                }
                if let Some(identity) = &options.workspace_identity {
                    command.env("ESCODE_WORKSPACE_IDENTITY", identity);
                }
                let mut child = command
                    .kill_on_drop(true)
                    .spawn()
                    .with_context(|| format!("Cannot start runtime child {}", binary.display()))?;
                let stdin = child.stdin.take().context("Runtime child stdin missing")?;
                let stdout = child
                    .stdout
                    .take()
                    .context("Runtime child stdout missing")?;
                Ok(Self {
                    reader: RuntimeReader::Child(BufReader::new(stdout)),
                    writer: RuntimeWriter::Child(stdin),
                    lifecycle: Lifecycle::Child(Box::new(ChildRuntime { child })),
                })
            }
        }
    }
}

impl RuntimeWriter {
    /// 写入一行请求（缺换行自动补）。
    pub async fn send_line(&mut self, line: &str) -> Result<()> {
        let mut bytes = line.as_bytes().to_vec();
        if bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        match self {
            Self::Embedded(writer) => {
                writer.send_line(bytes).await;
                Ok(())
            }
            Self::Child(stdin) => {
                stdin
                    .write_all(&bytes)
                    .await
                    .context("Runtime child stdin closed")?;
                stdin.flush().await.context("Runtime child flush failed")
            }
        }
    }
}

impl RuntimeReader {
    /// 读一行出站帧（含换行）；None = 链路关闭。
    pub async fn line(&mut self) -> Option<String> {
        match self {
            Self::Embedded(reader) => {
                let bytes: Vec<u8> = reader.line().await?;
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            Self::Child(stdout) => {
                let mut line = String::new();
                match stdout.read_line(&mut line).await {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line),
                }
            }
        }
    }
}

impl Lifecycle {
    /// 请求终止 child 进程（embedded 用取消令牌）。
    pub fn start_kill(&mut self) {
        if let Self::Child(runtime) = self {
            let _ = runtime.child.start_kill();
        }
    }

    /// 实例是否仍在运行（崩溃/退出后为 false → 上层懒重启下一代）。
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Embedded(runtime) => !runtime.task.is_finished(),
            Self::Child(runtime) => runtime.child.id().is_some(),
        }
    }

    /// 请求收口：embedded 走 cancel（与 SIGTERM 同路径）；child 终止进程。
    pub fn cancel(&mut self) {
        match self {
            Self::Embedded(runtime) => runtime.cancel.cancel(),
            Self::Child(_) => self.start_kill(),
        }
    }

    /// 收口后的残余任务清单（embedded 作用域；child 由操作系统回收进程树）。
    pub async fn wait(&mut self, grace: std::time::Duration) -> Result<Vec<String>> {
        match self {
            Self::Embedded(runtime) => {
                // 必须等实例任务真正结束：owner 锁（WorkspaceOwnerGuard）随实例任务
                // drop 释放，提前返回会让下一次 spawn 撞上「已被占用」。
                let _ = tokio::time::timeout(grace, &mut runtime.task).await;
                Ok(runtime.tasks.drain(grace).await)
            }
            Self::Child(runtime) => {
                let status = tokio::time::timeout(grace, runtime.child.wait())
                    .await
                    .ok()
                    .transpose()?;
                Ok(match status {
                    Some(_) => Vec::new(),
                    None => vec!["runtime-child".to_owned()],
                })
            }
        }
    }
}
