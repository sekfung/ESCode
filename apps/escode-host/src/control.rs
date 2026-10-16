//! 控制通路的进程侧实现：stdio 上的 NDJSON `ControlMessage` 读写、按实例的帧泵
//! （一个实例一个泵，向全部 attachment 扇出）与 `WorkspaceDescriptor` → `RuntimeOptions` 映射。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use escode_cli_rust::embed::RuntimeOptions;
use escode_host_control::logging::LogSink;
use escode_host_control::{ControlMessage, ControlWriter, WorkspaceDescriptor};
use escode_host_runtime::Instance;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

/// 控制队列容量（`rust-u2-host-skeleton.md` §3.2：控制必达、日志满即丢并计数）。
const CONTROL_QUEUE_CAPACITY: usize = 1024;

/// 控制通道：单写者 + 单读者（顺序即到达序）。
///
/// 出站走 [`ControlWriter`] 的**一条**有序队列：控制消息与控制循环、日志批次（服务域经
/// `logging` 的进程级 sink）都从这条队列出门，一个泵任务串行写 stdout——stdout 只有一个写者，
/// 行不会交错（§3.5）。日志满即丢、控制必达的语义由 `ControlWriter` 保证。
pub struct ControlLoop {
    /// `None` = 已收口（`close` 拿走最后一个句柄后队列关闭，泵写完即退）。
    writer: Option<ControlWriter>,
    pump: Option<tokio::task::JoinHandle<()>>,
    reader: tokio::io::Lines<BufReader<tokio::io::Stdin>>,
    /// 实例身份 → 帧泵（一个实例一个泵，扇出到全部 attachment）。
    pumps: BTreeMap<String, Pump>,
}

struct Pump {
    windows: Arc<AsyncMutex<BTreeSet<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl ControlLoop {
    pub fn start() -> Result<Self> {
        let (writer, mut messages) = ControlWriter::channel(CONTROL_QUEUE_CAPACITY);
        let pump = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(message) = messages.recv().await {
                if write_line(&mut stdout, &message).await.is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            writer: Some(writer),
            pump: Some(pump),
            reader: BufReader::new(tokio::io::stdin()).lines(),
            pumps: BTreeMap::new(),
        })
    }

    /// 服务域的日志 sink（进程级 `install_sink` 用）：日志与控制同一队列。
    pub fn log_sink(&self) -> LogSink {
        escode_host_control::logging::control_sink(self.writer().clone())
    }

    /// 出站句柄（`close` 前恒在）。
    /// 控制队列写句柄的克隆（给持有它的组件用；持有者必须在收口时释放，否则泵不退出）。
    pub fn writer_handle(&self) -> ControlWriter {
        self.writer().clone()
    }

    fn writer(&self) -> &ControlWriter {
        self.writer
            .as_ref()
            .expect("control writer is available before close()")
    }

    /// 读一条控制消息（EOF → None）。
    pub async fn recv(&mut self) -> Option<ControlMessage> {
        loop {
            let line = self.reader.next_line().await.ok().flatten()?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ControlMessage>(&line) {
                Ok(message) => return Some(message),
                Err(error) => {
                    // 未识别的控制行不致命：记录并继续（Main 可能带新字段）。
                    escode_host_control::logging::warn(
                        "host",
                        format!("ignored control line: {error}"),
                    );
                }
            }
        }
    }

    /// 写一条控制消息（单写者，行序即调用序）。
    pub async fn send(&self, message: ControlMessage) -> Result<()> {
        self.writer().control(message).await
    }

    /// 收口出站队列：**先释放进程级 sink**（它持有写句柄的克隆，不清掉队列永不关闭，
    /// 泵会一直等下一批日志），再拿走自己的写句柄，最后等泵把已入队的消息写完——
    /// 进程退出前的最后几帧（日志 + `stopped`）不能丢。之后的日志回退 stderr。
    pub async fn close(&mut self) {
        let Some(writer) = self.writer.take() else {
            return;
        };
        escode_host_control::logging::release_sink();
        drop(writer);
        if let Some(pump) = self.pump.take() {
            let _ = pump.await;
        }
    }

    /// 记录一个窗口接入某实例：首次为该实例启动帧泵，之后仅登记窗口。
    pub fn attach(&mut self, identity: &str, window_id: &str, instance: Arc<Instance>) {
        if let Some(pump) = self.pumps.get(identity) {
            let windows = Arc::clone(&pump.windows);
            let window = window_id.to_owned();
            tokio::spawn(async move {
                windows.lock().await.insert(window);
            });
            return;
        }
        let windows = Arc::new(AsyncMutex::new(BTreeSet::from([window_id.to_owned()])));
        let pump_windows = Arc::clone(&windows);
        let writer = self.writer().clone();
        let task = tokio::spawn(async move {
            // 帧泵：实例出站帧扇出到当前全部 attachment；窗口全断开时仍读取并丢弃
            // （丢弃实时帧与「attachment 只是视图」一致：会话事实在 runtime 侧，
            // 重连由订阅/快照恢复补齐）。
            let mut frames = instance.subscribe().await;
            while let Some(line) = frames.recv().await {
                let targets: Vec<String> = pump_windows.lock().await.iter().cloned().collect();
                for window_id in targets {
                    let message = ControlMessage::WindowFrame {
                        window_id,
                        line: line.clone(),
                    };
                    // 控制消息必达：队满时等待而不是丢弃（`ControlWriter::control`）。
                    if writer.control(message).await.is_err() {
                        return;
                    }
                }
            }
        });
        self.pumps
            .insert(identity.to_owned(), Pump { windows, task });
    }

    /// 断开一个窗口：只把它移出扇出集合。帧泵随实例存活——窗口全断开后帧被丢弃
    /// （attachment 只是视图，会话事实在 runtime 侧），重连由订阅/快照恢复补齐；
    /// 实例保持热态直到 `Shutdown` 或进程退出（窗口关闭不终止 runtime）。
    pub fn detach(&mut self, identity: &str, window_id: &str) {
        let Some(pump) = self.pumps.get(identity) else {
            return;
        };
        let windows = Arc::clone(&pump.windows);
        let window = window_id.to_owned();
        tokio::spawn(async move {
            windows.lock().await.remove(&window);
        });
    }

    /// 全部帧泵停止（Host 收口）。
    pub fn stop_pumps(&mut self) {
        for (_, pump) in std::mem::take(&mut self.pumps) {
            pump.task.abort();
        }
    }
}

/// 写一行 NDJSON 到 stdout（泵任务独占调用，行序即队列序）。
async fn write_line(writer: &mut tokio::io::Stdout, message: &ControlMessage) -> Result<()> {
    let line = serde_json::to_string(message)?;
    writer
        .write_all(line.as_bytes())
        .await
        .context("Host control stdout closed")?;
    writer.write_all(b"\n").await?;
    writer.flush().await.context("Host control flush failed")
}

/// `WorkspaceDescriptor` → 实例 `RuntimeOptions`：env 由 Main 提供（业务事实不从 Host
/// 进程 env 推断）。`home` 是实例进程的用户主目录（Node `os.homedir()` 语义：读
/// HOME/USERPROFILE），经 `core_api::env` 统一出口解析——crate 内不出现裸 `std::env`。
pub fn runtime_options(workspace: &WorkspaceDescriptor) -> RuntimeOptions {
    runtime_options_with(workspace, &BTreeMap::new())
}

/// 同上，并叠加 Main 经 `rpcListen.runtimeEnv` 提交的环境（审计 A16）：实例的 ambient env 只有这份快照，
/// 缺它 provider 配置文件、插件宿主入口等都读不到。`ESCODE_WORKSPACE_IDENTITY` 由 workspace 描述决定，优先于 Main 的同名项。
pub fn runtime_options_with(
    workspace: &WorkspaceDescriptor,
    runtime_env: &BTreeMap<String, String>,
) -> RuntimeOptions {
    let home = escode_cli_core_api::env::var_os("HOME")
        .or_else(|| escode_cli_core_api::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let mut env = runtime_env.clone();
    if let Some(identity) = &workspace.identity {
        env.insert("ESCODE_WORKSPACE_IDENTITY".to_owned(), identity.clone());
    }
    RuntimeOptions {
        home,
        env,
        argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
        cwd: std::path::PathBuf::from(&workspace.path),
        workspace_identity: workspace.identity.clone(),
        data_dir: workspace.data_dir.clone().map(std::path::PathBuf::from),
        config: workspace.config.clone().map(std::path::PathBuf::from),
    }
}

/// 实例身份键：`workspaceIdentity?.trim() || workspacePath`（AGENTS.md Workspace Identity）。
pub fn identity_of(workspace: &WorkspaceDescriptor) -> String {
    workspace
        .identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| workspace.path.clone())
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
