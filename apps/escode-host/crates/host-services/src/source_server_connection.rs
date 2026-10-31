//! 一条已建立的远端 server stdio 连接（S3，docs/specs/source-server-remote-rust-host.md §10.3）。
//!
//! 顺序固定：transport 启动命令 → `escode-hello` / `escode-hello-ack` → [`RemoteChannelClient`]。
//! 握手超时与 EOF 都返回显式错误，不静默重试；transport 差异只体现在 [`TransportCommand`]。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use escode_host_control::logging;
use escode_host_core::rpc_wire::FrameDecoder;

use crate::source_server_client::RemoteChannelClient;
use crate::source_server_handshake::{HandshakeReader, Hello, hello_ack_line};
use crate::source_server_transport::TransportCommand;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// stdout 读取块大小（与 Host 其它 stdio 泵一致）。
const READ_CHUNK: usize = 8192;
/// 握手失败时附带的 stderr 上限；只用于错误归因，不进入状态以外的持久化。
const STDERR_TAIL_CHARS: usize = 4096;

#[derive(Clone, Default)]
struct StderrTail(Arc<Mutex<String>>);

impl StderrTail {
    fn push(&self, line: &str) {
        let mut tail = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tail.push_str(line);
        tail.push('\n');
        if tail.len() > STDERR_TAIL_CHARS {
            let cut = tail.len() - STDERR_TAIL_CHARS;
            let cut = (cut..tail.len())
                .find(|index| tail.is_char_boundary(*index))
                .unwrap_or(tail.len());
            tail.drain(..cut);
        }
    }

    fn snapshot(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub struct ConnectOptions<'a> {
    pub client_id: &'a str,
    pub client_version: &'a str,
    pub handshake_timeout: Duration,
}

impl<'a> ConnectOptions<'a> {
    pub fn new(client_id: &'a str, client_version: &'a str) -> Self {
        Self {
            client_id,
            client_version,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }
}

/// 建立连接并完成握手；返回的 client 在 Initialize 之前也可安全排队请求。
pub struct RemoteConnection {
    pub client: std::sync::Arc<RemoteChannelClient>,
    pub hello: Hello,
    child: Child,
    tasks: Vec<JoinHandle<()>>,
    closed: watch::Sender<bool>,
}

impl RemoteConnection {
    /// 链路关闭信号：reader/writer 任一侧退出或显式 `shutdown` 后置位。
    ///
    /// 用 `watch` 而不是 `Notify`：观察者可能在关闭之后才订阅，`watch` 能立即看到已置位的值。
    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }

    /// 收口：拒绝挂起请求、终止子进程并等待退出。
    pub async fn shutdown(mut self) -> Result<()> {
        self.closed.send_replace(true);
        self.client.dispose();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await.context("wait for remote server")?;
        Ok(())
    }
}

pub async fn connect(
    command: &TransportCommand,
    options: &ConnectOptions<'_>,
) -> Result<RemoteConnection> {
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .envs(&command.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
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
            "spawned remote transport {:?}: pid={:?}",
            command.program,
            child.id()
        ),
    );
    let mut stdin = child
        .stdin
        .take()
        .context("remote transport stdin missing")?;
    let stdout = child
        .stdout
        .take()
        .context("remote transport stdout missing")?;
    let stderr_tail = StderrTail::default();
    let stderr_task = child.stderr.take().map(|stderr| {
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tail.push(&line);
                logging::warn("source-server", format!("remote: {line}"));
            }
        })
    });

    let handshake = match tokio::time::timeout(options.handshake_timeout, read_handshake(stdout))
        .await
    {
        Ok(Ok(handshake)) => handshake,
        Ok(Err(error)) => {
            return Err(terminate_with_stderr(&mut child, stderr_task, &stderr_tail, error).await);
        }
        Err(_) => {
            let error = anyhow::anyhow!(
                "handshake timeout: no escode-hello within {:?}",
                options.handshake_timeout
            );
            return Err(terminate_with_stderr(&mut child, stderr_task, &stderr_tail, error).await);
        }
    };

    let ack = hello_ack_line(options.client_version, options.client_id)?;
    stdin
        .write_all(ack.as_bytes())
        .await
        .context("send hello ack")?;
    stdin.flush().await.context("flush hello ack")?;

    let (outbound, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
    let client = std::sync::Arc::new(RemoteChannelClient::new(outbound));
    let (closed_tx, _) = watch::channel(false);
    let writer_closed = closed_tx.clone();
    let writer = tokio::spawn(async move {
        while let Some(bytes) = frames.recv().await {
            if stdin.write_all(&bytes).await.is_err() {
                writer_closed.send_replace(true);
                return;
            }
            let _ = stdin.flush().await;
        }
    });
    let reader_client = std::sync::Arc::clone(&client);
    let reader_closed = closed_tx.clone();
    let reader = tokio::spawn(async move {
        let mut decoder = FrameDecoder::new();
        let mut stdout = handshake.stdout;
        if !handshake.remaining.is_empty() {
            decoder.push(&handshake.remaining);
        }
        loop {
            loop {
                match decoder.next_frame() {
                    Ok(Some(frame)) => {
                        if reader_client.handle_frame(&frame).is_err() {
                            reader_client.dispose();
                            reader_closed.send_replace(true);
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        logging::warn("source-server", format!("frame decode failed: {error}"));
                        reader_client.dispose();
                        reader_closed.send_replace(true);
                        return;
                    }
                }
            }
            let mut chunk = [0u8; READ_CHUNK];
            match stdout.read(&mut chunk).await {
                Ok(0) | Err(_) => {
                    reader_client.dispose();
                    reader_closed.send_replace(true);
                    return;
                }
                Ok(read) => decoder.push(&chunk[..read]),
            }
        }
    });
    let mut tasks = vec![writer, reader];
    if let Some(stderr_task) = stderr_task {
        tasks.push(stderr_task);
    }
    Ok(RemoteConnection {
        client,
        hello: handshake.hello,
        child,
        tasks,
        closed: closed_tx,
    })
}

/// 终止未完成握手的子进程并等待 stderr 排水，把 SSH 的主机 key 诊断带进错误链。
async fn terminate_with_stderr(
    child: &mut Child,
    stderr_task: Option<JoinHandle<()>>,
    stderr_tail: &StderrTail,
    error: anyhow::Error,
) -> anyhow::Error {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    if let Some(task) = stderr_task {
        let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
    }
    let stderr = stderr_tail.snapshot();
    let stderr = stderr.trim();
    if stderr.is_empty() {
        error
    } else {
        error.context(format!("remote stderr: {stderr}"))
    }
}

struct PendingHandshake {
    hello: Hello,
    remaining: Vec<u8>,
    stdout: tokio::process::ChildStdout,
}

async fn read_handshake(mut stdout: tokio::process::ChildStdout) -> Result<PendingHandshake> {
    let mut reader = HandshakeReader::new();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        if let Some(handshake) = reader.poll().map_err(anyhow::Error::new)? {
            return Ok(PendingHandshake {
                hello: handshake.hello,
                remaining: handshake.remaining,
                stdout,
            });
        }
        match stdout.read(&mut chunk).await {
            Ok(0) => bail!("remote transport closed before escode-hello"),
            Ok(read) => reader.push(&chunk[..read]),
            Err(error) => return Err(error).context("read remote handshake"),
        }
    }
}

#[cfg(test)]
#[path = "source_server_connection_tests.rs"]
mod tests;
