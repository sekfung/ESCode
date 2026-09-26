//! 后台 Bash 任务：显式后台与超时自动转后台共用登记与收尾（docs/specs/rust-bash-auto-background.md）。
use super::super::tool_process::{BACKGROUNDED, FOREGROUND, ShellContext, run, shell_output};
use super::{Job, ShellTasks};
use crate::{
    contract::{Event, EventSink, ToolOutput},
    domain::background::BackgroundTask,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// 一次 Bash 启动所需的全部归属数据；进程任务与收尾任务共享。
pub(in super::super) struct Launch {
    pub cwd: PathBuf,
    pub artifacts: PathBuf,
    pub session: String,
    pub id: String,
    pub path: PathBuf,
    pub combined: Arc<Mutex<tokio::fs::File>>,
    pub shell: Option<crate::shell_select::Override>,
    pub command: String,
    pub description: String,
    pub lifecycle: AtomicU8,
}

type Running = JoinHandle<Result<Value>>;

impl Launch {
    fn spawn(self: &Arc<Self>, timeout: Option<Duration>, token: CancellationToken) -> Running {
        let launch = self.clone();
        tokio::spawn(async move {
            let context = ShellContext {
                over: launch.shell.as_ref(),
                startup_root: &launch.artifacts,
                session: &launch.session,
                lifecycle: &launch.lifecycle,
            };
            run(&launch.cwd, &launch.command, &launch.path, launch.combined.clone(), timeout, &token, context).await
        })
    }
    fn task(&self, sink: &EventSink) -> BackgroundTask {
        BackgroundTask {
            id: self.id.clone(),
            run_id: sink.run_id.clone(),
            title: self.description.clone(),
            status: "running".into(),
            started_at: super::super::now(),
            ended_at: None,
            output_file: self.path.to_string_lossy().into_owned(),
        }
    }
    fn backgrounded(&self) -> ToolOutput {
        shell_output(json!({"stdout":"","stderr":"","status":"backgrounded","interrupted":false,"backgroundTaskId":self.id,"rawOutputPath":self.path,"persistedOutputPath":self.path,"backgroundedByUser":false}))
    }
}

impl ShellTasks {
    /// 显式后台：先登记（owner 提交回执）再启动进程。
    pub(super) async fn start_background(
        &self,
        launch: Arc<Launch>,
        sink: &EventSink,
        timeout: Option<Duration>,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let token = CancellationToken::new();
        let (tx, state) = watch::channel(None);
        let task = launch.task(sink);
        self.register(&launch, sink, &task, &token, state, &tx, cancel).await?;
        if cancel.is_cancelled() {
            token.cancel();
        }
        let running = launch.spawn(timeout, token);
        tokio::spawn(finish(running, sink.clone(), task, tx));
        Ok(launch.backgrounded())
    }

    /// 超时自动转后台（TS auto_on_timeout）：进程无超时运行，期限到时才登记为后台任务。
    pub(super) async fn start_auto(
        &self,
        launch: Arc<Launch>,
        sink: &EventSink,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let token = CancellationToken::new();
        // 转后台之前工具 future 被丢弃时回收进程，不留孤儿。
        let guard = token.clone().drop_guard();
        let mut running = launch.spawn(None, token.clone());
        tokio::select! {biased;
            result = &mut running => return Ok(shell_output(result??)),
            _ = cancel.cancelled() => {
                token.cancel();
                return Ok(shell_output(running.await??));
            }
            _ = tokio::time::sleep(timeout) => {}
        }
        // 与进程结束竞争归属：进程已结算为前台结果时按前台返回。
        if launch
            .lifecycle
            .compare_exchange(FOREGROUND, BACKGROUNDED, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(shell_output(running.await??));
        }
        let (tx, state) = watch::channel(None);
        let task = launch.task(sink);
        if let Err(error) = self.register(&launch, sink, &task, &token, state, &tx, cancel).await {
            token.cancel();
            let _ = running.await;
            let after = crate::domain::bash_model_content::timeout_duration(timeout.as_millis() as u64);
            bail!("Command timed out after {after} and could not move to the background: {error:#}");
        }
        guard.disarm();
        tokio::spawn(finish(running, sink.clone(), task, tx));
        Ok(launch.backgrounded())
    }

    /// 登记后台任务：上限检查、写入会话任务表、Background running 并等待 owner 提交。
    #[allow(clippy::too_many_arguments)]
    async fn register(
        &self,
        launch: &Launch,
        sink: &EventSink,
        task: &BackgroundTask,
        token: &CancellationToken,
        state: watch::Receiver<Option<Value>>,
        tx: &watch::Sender<Option<Value>>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let job = Arc::new(Job {
            cancel: token.clone(),
            state,
            path: launch.path.clone(),
            command: launch.command.clone(),
            description: launch.description.clone(),
        });
        {
            let mut all = self.jobs.lock().await;
            let jobs = all.entry(launch.session.clone()).or_default();
            if jobs.values().filter(|j| j.state.borrow().is_none()).count() >= 16 {
                bail!("Background task limit (16) reached");
            }
            if jobs.len() >= 128
                && let Some(old) = jobs.iter().find(|(_, j)| j.state.borrow().is_some()).map(|(id, _)| id.clone())
            {
                jobs.remove(&old);
            }
            jobs.insert(launch.id.clone(), job);
        }
        let (committed, receipt) = oneshot::channel();
        let registered = async {
            sink.send(Event::Background { task: task.clone(), committed: Some(committed) }).await?;
            receipt.await.context("Background registration was not committed")?;
            Ok::<_, anyhow::Error>(())
        };
        let registered = tokio::select! {_=cancel.cancelled()=>Err(anyhow::anyhow!("Cancelled")),r=registered=>r};
        if let Err(e) = registered {
            // owner 可能已经提交 running、但工具尚未收到回执；取消时也要投递终态，
            // 否则 close/EOF 会永远等待一个从未 spawn 的后台任务。
            let mut terminal = task.clone();
            terminal.status = if cancel.is_cancelled() { "cancelled" } else { "failed" }.into();
            terminal.ended_at = Some(super::super::now());
            let _ = sink.send(Event::Background { task: terminal, committed: None }).await;
            let _ = tx.send(Some(json!({"status":"cancelled","interrupted":true})));
            if let Some(jobs) = self.jobs.lock().await.get_mut(&launch.session) {
                jobs.remove(&launch.id);
            }
            return Err(e);
        }
        Ok(())
    }
}

/// 进程结束后投递 Background 终态，再允许 TaskOutput/TaskStop 返回；同一通道保持提交先于工具结果。
async fn finish(running: Running, sink: EventSink, mut task: BackgroundTask, tx: watch::Sender<Option<Value>>) {
    let result = running.await.map_err(anyhow::Error::from).and_then(|r| r);
    if let Err(error) = &result
        && error.is::<crate::contract::ProcessCleanupFailure>()
    {
        let _ = sink.send(Event::ToolCleanupFailed(format!("{error:#}"))).await;
    }
    let result = result
        .unwrap_or_else(|e| json!({"stdout":"","stderr":e.to_string(),"status":"spawn_error","interrupted":false}));
    task.ended_at = Some(super::super::now());
    task.status = match result["status"].as_str() {
        Some("completed") => "completed",
        Some("cancelled") => "cancelled",
        _ => "failed",
    }
    .into();
    let _ = sink.send(Event::Background { task, committed: None }).await;
    let _ = tx.send(Some(result));
}
