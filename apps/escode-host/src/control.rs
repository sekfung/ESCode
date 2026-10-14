//! 控制通路的进程侧实现：stdio 上的 NDJSON `ControlMessage` 读写、按实例的帧泵
//! （一个实例一个泵，向全部 attachment 扇出）与 `WorkspaceDescriptor` → `RuntimeOptions` 映射。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use escode_cli_rust::embed::RuntimeOptions;
use escode_host_control::{ControlMessage, WorkspaceDescriptor};
use escode_host_runtime::Instance;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// 控制通道：单写者 + 单读者（顺序即到达序）。
pub struct ControlLoop {
    writer: Arc<Mutex<tokio::io::Stdout>>,
    reader: tokio::io::Lines<BufReader<tokio::io::Stdin>>,
    /// 实例身份 → 帧泵（一个实例一个泵，扇出到全部 attachment）。
    pumps: BTreeMap<String, Pump>,
}

struct Pump {
    windows: Arc<Mutex<BTreeSet<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl ControlLoop {
    pub fn start() -> Result<Self> {
        Ok(Self {
            writer: Arc::new(Mutex::new(tokio::io::stdout())),
            reader: BufReader::new(tokio::io::stdin()).lines(),
            pumps: BTreeMap::new(),
        })
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
                    eprintln!("escode-host: ignored control line: {error}");
                }
            }
        }
    }

    /// 写一条控制消息（单写者，行序即调用序）。
    pub async fn send(&self, message: ControlMessage) -> Result<()> {
        write_message(&self.writer, &message).await
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
        let windows = Arc::new(Mutex::new(BTreeSet::from([window_id.to_owned()])));
        let pump_windows = Arc::clone(&windows);
        let writer = Arc::clone(&self.writer);
        let task = tokio::spawn(async move {
            // 帧泵：实例出站帧扇出到当前全部 attachment；窗口全断开时仍读取并丢弃
            // （丢弃实时帧与「attachment 只是视图」一致：会话事实在 runtime 侧，
            // 重连由订阅/快照恢复补齐）。
            while let Some(line) = instance.line().await {
                let targets: Vec<String> = pump_windows.lock().await.iter().cloned().collect();
                for window_id in targets {
                    let message = ControlMessage::WindowFrame {
                        window_id,
                        line: line.clone(),
                    };
                    if write_message(&writer, &message).await.is_err() {
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

async fn write_message(
    writer: &Arc<Mutex<tokio::io::Stdout>>,
    message: &ControlMessage,
) -> Result<()> {
    let line = serde_json::to_string(message)?;
    let mut writer = writer.lock().await;
    writer
        .write_all(line.as_bytes())
        .await
        .context("Host control stdout closed")?;
    writer.write_all(b"\n").await?;
    writer.flush().await.context("Host control flush failed")
}

/// `WorkspaceDescriptor` → 实例 `RuntimeOptions`：env 由 Main 提供（业务事实不从 Host
/// 进程 env 推断），HOME 缺省取进程 HOME（组合根职责，与 CLI main.rs 同）。
pub fn runtime_options(workspace: &WorkspaceDescriptor) -> RuntimeOptions {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let mut env = BTreeMap::new();
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
