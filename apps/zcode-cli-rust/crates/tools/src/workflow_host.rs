//! 动态工作流宿主客户端（docs/specs/rust-dynamic-workflow.md「运行面实现计划」M1）。
//!
//! 经 Host 的 Node 启动器常驻一个 `__zcode-workflow-host` 子进程：它复用 TS 的 run 服务与工作流工具
//! handler，journal 直接写本进程会话库的 dwf_* 表。请求按 id 多路复用（可并发）；宿主的 `runSettled`
//! 通知经工具层 Host 通道变成 `Event::WorkflowSettled`，由会话 owner 在空闲时注入后台结果轮。
//! 子进程退出时所有在途请求失败，下一次调用重新拉起。

use super::workflow_analyzer::launcher;
use crate::contract::{Event, EventSink};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, oneshot};

const COMMAND: &str = "__zcode-workflow-host";
/// 宿主负责的工作流工具（其余由 Rust 原生实现）。
pub(crate) const TOOLS: [&str; 5] = [
    "CreateWorkflow",
    "AmendWorkflow",
    "ResumeWorkflowRun",
    "GetWorkflowRun",
    "ResolveWorkflowQuestion",
];

/// 交给宿主执行的调用：宿主工具，以及指向工作流 run 的 TaskStop / TaskOutput（TS 后台任务控制端口的
/// `local_dynamic_workflow` 分支与运行时任务注册表，见宿主的 tracker）。
pub(crate) fn routes(name: &str, args: &Value) -> bool {
    TOOLS.contains(&name)
        || matches!(name, "TaskStop" | "TaskOutput")
            && args["task_id"]
                .as_str()
                .or(args["shell_id"].as_str())
                .is_some_and(|id| id.starts_with("dwfrun-"))
}

type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

struct Process {
    _child: Child,
    stdin: ChildStdin,
}

#[derive(Default)]
pub(crate) struct WorkflowHost {
    process: Arc<Mutex<Option<Process>>>,
    pending: Pending,
    next_id: std::sync::atomic::AtomicU64,
    db_path: OnceLock<PathBuf>,
    host: Arc<OnceLock<EventSink>>,
}

impl WorkflowHost {
    pub(crate) fn set_db_path(&self, path: PathBuf) {
        let _ = self.db_path.set(path);
    }

    pub(crate) fn attach_host(&self, sink: EventSink) {
        let _ = self.host.set(sink);
    }

    /// 发一个请求并等应答。宿主不可用（无启动器 / 无会话库）时报错。
    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let (reply, receipt) = oneshot::channel();
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        {
            let mut guard = self.process.lock().await;
            if guard.is_none() {
                *guard = Some(self.spawn().await?);
            }
            self.pending.lock().unwrap().insert(id, reply);
            let line = json!({ "id": id, "method": method, "params": params }).to_string();
            let process = guard.as_mut().expect("spawned workflow host");
            let written = async {
                process.stdin.write_all(line.as_bytes()).await?;
                process.stdin.write_all(b"\n").await?;
                process.stdin.flush().await
            }
            .await;
            if let Err(error) = written {
                self.pending.lock().unwrap().remove(&id);
                *guard = None;
                return Err(anyhow!("Workflow host is unavailable: {error}"));
            }
        }
        match receipt.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => bail!("{message}"),
            Err(_) => bail!("Workflow host exited before answering"),
        }
    }

    /// 不等应答的通知（actor 运行事件）：宿主未起时直接丢弃。
    pub(crate) fn notify(self: &Arc<Self>, method: &'static str, params: Value) {
        let this = self.clone();
        tokio::spawn(async move {
            if this.process.lock().await.is_some() {
                let _ = this.request(method, params).await;
            }
        });
    }

    async fn spawn(&self) -> Result<Process> {
        let (exec, entrypoint) = launcher()
            .context("Workflow execution is unavailable: the Host did not provide a Node launcher")?;
        let db_path = self
            .db_path
            .get()
            .cloned()
            .context("Workflow execution is unavailable: no session database")?;
        let mut command = tokio::process::Command::new(exec);
        command
            .arg(entrypoint)
            .arg(COMMAND)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        zcode_cli_host::child_env::apply(&mut command, false);
        command.env("ELECTRON_RUN_AS_NODE", "1");
        let mut child = command.spawn().context("Failed to start the workflow host")?;
        let mut stdin = child.stdin.take().context("workflow host stdin")?;
        let stdout = child.stdout.take().context("workflow host stdout")?;
        let (pending, host) = (self.pending.clone(), self.host.clone());
        let process = self.process.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = message["id"].as_u64() {
                    if let Some(reply) = pending.lock().unwrap().remove(&id) {
                        let _ = reply.send(match message["error"].as_str() {
                            Some(error) => Err(error.to_owned()),
                            None => Ok(message["result"].clone()),
                        });
                    }
                    continue;
                }
                // 宿主对 actor 会话的请求：交给会话 owner，应答以 `{replyTo, result | error}` 写回宿主。
                if message["event"] == "request"
                    && let (Some(id), Some(sink)) = (message["id"].as_str(), host.get())
                {
                    let (reply, receipt) = oneshot::channel();
                    let event = Event::ActorRequest {
                        method: message["method"].as_str().unwrap_or_default().to_owned(),
                        params: message["params"].clone(),
                        reply,
                    };
                    let (id, sink, process) = (id.to_owned(), sink.clone(), process.clone());
                    tokio::spawn(async move {
                        let answer = if sink.send(event).await.is_err() {
                            Err("Rust runtime is shutting down".to_owned())
                        } else {
                            receipt
                                .await
                                .unwrap_or_else(|_| Err("Actor request dropped".to_owned()))
                        };
                        let line = match answer {
                            Ok(result) => json!({ "replyTo": id, "result": result }),
                            Err(error) => json!({ "replyTo": id, "error": error }),
                        };
                        if let Some(process) = process.lock().await.as_mut() {
                            let _ = process
                                .stdin
                                .write_all(format!("{line}
").as_bytes())
                                .await;
                            let _ = process.stdin.flush().await;
                        }
                    });
                    continue;
                }
                // 结算与 run 中通知（升级问答 / 停滞）同一条投递路（后台结果轮）。
                if (message["event"] == "runSettled" || message["event"] == "runNotice")
                    && let Some(sink) = host.get()
                {
                    let params = message["params"].clone();
                    let session = params["session"].as_str().unwrap_or_default().to_owned();
                    let _ = sink.send(Event::WorkflowSettled { session, notice: params }).await;
                }
            }
            // 子进程退出：在途请求全部失败（下一次调用重新拉起）。
            pending.lock().unwrap().clear();
        });
        // init 是第一条请求：打开会话库（只碰 dwf_* 表）。
        let init = json!({ "id": 0, "method": "init", "params": { "dbPath": db_path.to_string_lossy() } });
        stdin.write_all(format!("{init}\n").as_bytes()).await?;
        stdin.flush().await?;
        Ok(Process { _child: child, stdin })
    }
}
