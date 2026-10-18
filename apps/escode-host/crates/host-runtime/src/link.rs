//! runtime-link：Host 认识 runtime 的唯一边界（rust-u2-host-skeleton.md §3 / §5 第 9 项）。
//!
//! 两种链路对上层同形（`send_line` / `line`）：
//! - `Embedded`：同进程 `escode_cli_rust::embed::spawn`（阶段 1 库化产物，默认）；
//! - `Child`：`app-server --stdio` 子进程（差分、回退与故障隔离选项）。
//!
//! 链路上的帧面与 `app-server --stdio` 完全一致（NDJSON），所以链路之上代码不区分传输。

use escode_host_control::logging;
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

/// Windows 子进程启动所必需的系统变量（平台底座，不是业务事实）。
///
/// `Command::env_clear` 在 Windows 上会一并清掉 `SystemRoot`；Win32 的 socket、
/// 系统证书库等会在缺少它时失败（rust-lang/rust#114737，Go 的 `exec.Cmd` 也总是
/// 补 `SYSTEMROOT`）。`RuntimeOptions.env` 显式给出的同名值优先。
#[cfg(windows)]
const WINDOWS_BASE_ENV: &[&str] = &[
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "TEMP",
    "TMP",
    "PATH",
];

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
    /// 实例任务的结束原因（已 join 过）。`JoinHandle` 完成后不能再 poll，
    /// 崩溃回收与随后的 `shutdown` 都会调 [`Lifecycle::wait`]，第二次直接用这里的结果。
    joined: Option<String>,
}

/// 一次收口/回收的结果：退出原因 + 宽限后的残余清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub cause: String,
    pub residual: Vec<String>,
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
                        joined: None,
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
                #[cfg(windows)]
                for name in WINDOWS_BASE_ENV {
                    // 快照里显式给出的值优先（Windows 环境变量名大小写不敏感）。
                    if options
                        .env
                        .keys()
                        .any(|key| key.eq_ignore_ascii_case(*name))
                    {
                        continue;
                    }
                    if let Some(value) = escode_cli_core_api::env::var_os(*name) {
                        command.env(*name, value);
                    }
                }
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
                // 修复：stderr 先前被 piped 却无人读取，子进程日志写满管道缓冲（约 64 KiB）后
                // 会阻塞在 write 上、整个实例停摆。这里逐行转发到 Host 的 stderr（与 Host 日志同一出口）。
                if let Some(stderr) = child.stderr.take() {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            logging::warn("runtime-link", format!("runtime-child: {line}"));
                        }
                    });
                }
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
    /// child 链路的子进程 pid（embedded 为 None：实例与 Host 同进程）。
    pub fn process_id(&self) -> Option<u32> {
        match self {
            Self::Embedded(_) => None,
            Self::Child(runtime) => runtime.child.id(),
        }
    }

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

    /// 收口/回收：等实例结束（宽限内），返回退出原因与残余清单（embedded 作用域；child 由操作系统
    /// 回收进程树）。幂等：崩溃回收之后再被 `shutdown` 调用时返回同一原因，不重复 poll 已完成的任务。
    pub async fn wait(&mut self, grace: std::time::Duration) -> Result<Exit> {
        match self {
            Self::Embedded(runtime) => {
                // 必须等实例任务真正结束：owner 锁（WorkspaceOwnerGuard）随实例任务
                // drop 释放，提前返回会让下一次 spawn 撞上「已被占用」。
                if runtime.joined.is_none() {
                    match tokio::time::timeout(grace, &mut runtime.task).await {
                        Ok(joined) => {
                            runtime.joined = Some(match joined {
                                Ok(Ok(())) => "exited".to_owned(),
                                Ok(Err(error)) => format!("{error:#}"),
                                Err(error) if error.is_panic() => {
                                    format!("panicked: {error}")
                                }
                                Err(error) => format!("aborted: {error}"),
                            });
                        }
                        Err(_) => {
                            return Ok(Exit {
                                cause: format!("did not stop within {grace:?}"),
                                residual: runtime.tasks.drain(grace).await,
                            });
                        }
                    }
                }
                Ok(Exit {
                    cause: runtime.joined.clone().unwrap_or_default(),
                    residual: runtime.tasks.drain(grace).await,
                })
            }
            Self::Child(runtime) => {
                // tokio 的 `Child::wait` 在进程回收后返回缓存的状态，可重复调用。
                let status = tokio::time::timeout(grace, runtime.child.wait())
                    .await
                    .ok()
                    .transpose()?;
                Ok(match status {
                    Some(status) => Exit {
                        cause: status.to_string(),
                        residual: Vec::new(),
                    },
                    None => Exit {
                        cause: format!("did not stop within {grace:?}"),
                        residual: vec!["runtime-child".to_owned()],
                    },
                })
            }
        }
    }
}
