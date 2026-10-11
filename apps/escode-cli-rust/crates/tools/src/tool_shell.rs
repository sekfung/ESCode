use super::tool_process::{BACKGROUNDED, FOREGROUND, ShellContext, run, shell_output};
#[path = "tool_shell_jobs.rs"]
mod jobs;
use super::tools::{boolean, keys, string, truncate_utf8, uint};
use crate::{
    contract::{Event, EventSink, ToolOutput},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU8},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::{Mutex, oneshot, watch},
};
use tokio_util::sync::CancellationToken;
struct Job {
    cancel: CancellationToken,
    state: watch::Receiver<Option<Value>>,
    path: PathBuf,
    command: String,
    description: String,
}
#[derive(Default)]
pub struct ShellTasks {
    jobs: Mutex<HashMap<String, HashMap<String, Arc<Job>>>>,
}
impl ShellTasks {
    pub async fn call(
        &self,
        paths: (&Path, &Path, &Path),
        session: &str,
        name: &str,
        args: &Value,
        sink: Option<&EventSink>,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        match name {
            "Bash" => {
                self.start(paths, session, args, sink, cancel)
                    .await
            }
            "TaskOutput" | "TaskStop" => {
                keys(
                    args,
                    if name == "TaskOutput" {
                        &["task_id", "block", "timeout"]
                    } else {
                        &["task_id", "shell_id"]
                    },
                )?;
                let id = args["task_id"]
                    .as_str()
                    .or_else(|| args["shell_id"].as_str())
                    .context("task_id required")?;
                // TS：进程恢复后任务不重启，注册表里也就没有它；文案同 TS task-output / task-stop。
                let Some(job) = self.jobs.lock().await.get(session).and_then(|jobs| jobs.get(id)).cloned() else {
                    return Err(crate::domain::file_tool_text::task_not_found(name, id));
                };
                let mut state = job.state.clone();
                if name == "TaskStop" {
                    job.cancel.cancel();
                    if state.borrow().is_none() {
                        tokio::select! {_=cancel.cancelled()=>bail!("Cancelled"),r=state.wait_for(|v|v.is_some())=>{r?;}}
                    }
                    // TS task-stop：输出对象按 JSON 原样交给模型（键序 message / task_id / task_type / command）。
                    let message = format!("Successfully stopped task: {id} ({})", job.command);
                    let data = json!({"message":message,"task_id":id,"task_type":"local_bash","command":job.command});
                    let content = format!(
                        r#"{{"message":{},"task_id":{},"task_type":"local_bash","command":{}}}"#,
                        json!(message),
                        json!(id),
                        json!(job.command)
                    );
                    return Ok(ToolOutput {
                        media: Vec::new(),
                        failed: false,
                        content,
                        display: Some(
                            json!({"kind":"task_stop","taskId":id,"taskType":"bash","command":job.command,"message":message}),
                        ),
                        data,
                        control: Default::default(),
                    });
                }
                let timeout = uint(args, "timeout", 30000)?;
                if timeout > 600000 {
                    bail!("TaskOutput timeout exceeds 600000 ms");
                }
                let mut retrieval = "success";
                if state.borrow().is_none() {
                    if boolean(args, "block", true)? {
                        tokio::select! {
                            _=cancel.cancelled()=>bail!("Cancelled"),
                            result=tokio::time::timeout(Duration::from_millis(timeout),state.wait_for(|s|s.is_some()))=>{match result{Ok(r)=>{r?;},Err(_)=>retrieval="timeout"}},
                        }
                    } else {
                        retrieval = "not_ready";
                    }
                }
                let final_result = state.borrow().clone();
                // TS projectBashTask：运行中读文件头 30000 字节，终态读尾部 8 MiB；字符预算由模型面格式器施加。
                let mut file = tokio::fs::File::open(&job.path).await?;
                let size = file.metadata().await?.len();
                let mut bytes = vec![];
                if final_result.is_none() {
                    (&mut file).take(30_000).read_to_end(&mut bytes).await?;
                } else {
                    let tail = size.min(8 * 1024 * 1024);
                    file.seek(std::io::SeekFrom::Start(size - tail)).await?;
                    file.read_to_end(&mut bytes).await?;
                }
                let output = escode_cli_host::output_encoding::decode_output(&bytes);
                let status = final_result
                    .as_ref()
                    .map(|v| match v["status"].as_str() {
                        Some("completed") => "completed",
                        Some("cancelled") => "killed",
                        _ => "failed",
                    })
                    .unwrap_or("running");
                let data = json!({"retrieval_status":retrieval,"task":{"task_id":id,"task_type":"local_bash","status":status,"description":job.description,"output":output,"exitCode":final_result.as_ref().and_then(|v|v["exitCode"].as_i64()),"outputFile":job.path}});
                let mut preview = output.clone();
                truncate_utf8(&mut preview, 1800);
                let mut display =
                    json!({"kind":"task_output","retrievalStatus":retrieval,"taskStatus":status});
                if !preview.is_empty() {
                    display["output"] = preview.into();
                }
                if output.len() > 1800 {
                    display["truncated"] = true.into();
                }
                Ok(ToolOutput {
                    media: Vec::new(),
                    failed: false,
                    content: crate::domain::task_output::model_content(&data),
                    data,
                    display: Some(display),
                    control: Default::default(),
                })
            }
            _ => bail!("Unsupported shell tool"),
        }
    }
    async fn start(
        &self,
        paths: (&Path, &Path, &Path),
        session: &str,
        args: &Value,
        sink: Option<&EventSink>,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let (cwd, artifacts, workspace) = paths;
        keys(
            args,
            &[
                "command",
                "description",
                "timeout",
                "run_in_background",
                "dangerouslyDisableSandbox",
                crate::domain::off_peak::FOREGROUND_ONLY_ARG,
                crate::domain::background::TOOL_CALL_ID_ARG,
            ],
        )?;
        // core 在闲时受限轮加入的内部参数：关闭超时自动转后台（docs/specs/rust-offpeak.md 第二期）。
        let foreground_only = args[crate::domain::off_peak::FOREGROUND_ONLY_ARG] == true;
        let command = string(args, "command")?.to_owned();
        if command.trim().is_empty() {
            bail!("command must not be empty");
        }
        let background = boolean(args, "run_in_background", false)?;
        boolean(args, "dangerouslyDisableSandbox", false)?;
        let description = args
            .get("description")
            .map(|_| string(args, "description"))
            .transpose()?
            .unwrap_or(&command)
            .to_owned();
        let timeout = match args.get("timeout") {
            None if background => None,
            value => {
                let n = match value {
                    None => 120000.0,
                    Some(v) => v
                        .as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                        .context("Invalid Bash timeout")?,
                };
                if !n.is_finite() || n < 0.0 {
                    bail!("Invalid Bash timeout");
                }
                Some(Duration::from_millis(if n == 0.0 {
                    120000
                } else {
                    (n as u64).min(600000)
                }))
            }
        };
        let shell = shell_override(sink).await;
        tokio::fs::create_dir_all(artifacts).await?;
        // TS node-execution-adapter：任务 id `exec_<uuid>`，合并输出落 `<toolCallId>-stdout.log`（id 经 sanitizePathSegment）。
        let id = format!("exec_{}", super::id());
        let file = args[crate::domain::background::TOOL_CALL_ID_ARG]
            .as_str()
            .map(|call| {
                let safe: String = call
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
                    .take(120)
                    .collect();
                if safe.is_empty() { "unknown".to_owned() } else { safe }
            })
            .unwrap_or_else(super::id);
        let path = artifacts.join(format!("{file}-stdout.log"));
        let combined = Arc::new(Mutex::new(tokio::fs::File::create(&path).await?));
        let lifecycle = if background { BACKGROUNDED } else { FOREGROUND };
        let launch = Arc::new(jobs::Launch {
            cwd: cwd.to_owned(),
            artifacts: artifacts.to_owned(),
            workspace: workspace.to_owned(),
            session: session.to_owned(),
            id,
            path,
            combined,
            shell,
            tool_call_id: args[crate::domain::background::TOOL_CALL_ID_ARG].as_str().map(str::to_owned),
            raw_description: args["description"].as_str().map(str::to_owned),
            command,
            description,
            lifecycle: AtomicU8::new(lifecycle),
        });
        // TS isBashAutoBackgroundEligible：以 sleep 开头的命令超时即终止，不转后台。
        let auto = !foreground_only && launch.command.split_whitespace().next() != Some("sleep");
        match (background, sink) {
            (true, sink) => {
                let sink = sink.context("Background execution requires a session owner")?;
                self.start_background(launch, sink, timeout, cancel).await
            }
            (false, Some(sink)) if auto => {
                self.start_auto(launch, sink, timeout.context("Invalid Bash timeout")?, cancel)
                    .await
            }
            (false, _) => {
                let context = ShellContext {
                    over: launch.shell.as_ref(),
                    startup_root: artifacts,
                    session,
                    lifecycle: &launch.lifecycle,
                    workspace: &launch.workspace,
                };
                let data = run(
                    cwd,
                    &launch.command,
                    &launch.path,
                    launch.combined.clone(),
                    timeout,
                    cancel,
                    context,
                )
                .await?;
                Ok(shell_output(data))
            }
        }
    }
    /// `v4/conversation/backgroundBashOutput`（TS node-execution-adapter readBackgroundBashOutput）：后台详情的
    /// 输出尾窗（≤ 8192 字节）。任务按启动会话校验归属；先冻结状态再读文件。
    pub async fn output(&self, session: &str, work_id: &str) -> Value {
        let job = self.jobs.lock().await.get(session).and_then(|jobs| jobs.get(work_id)).cloned();
        let Some(job) = job else {
            return json!({ "kind": "unavailable", "workId": work_id });
        };
        let status = match job.state.borrow().as_ref().map(|r| r["status"].as_str().unwrap_or("failed").to_owned()) {
            None => "running".to_owned(),
            Some(s) if ["completed", "failed", "timed_out", "cancelled", "spawn_error"].contains(&s.as_str()) => s,
            Some(_) => "failed".to_owned(),
        };
        let read = async {
            let mut file = tokio::fs::File::open(&job.path).await?;
            let size = file.metadata().await?.len();
            let length = size.min(8192);
            file.seek(std::io::SeekFrom::Start(size - length)).await?;
            let mut bytes = vec![];
            file.read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>((escode_cli_host::output_encoding::decode_output(&bytes), size > bytes.len() as u64))
        };
        match read.await {
            Ok((output, truncated)) => json!({
                "kind": "output", "workId": work_id, "status": status, "output": output,
                "truncated": truncated, "outputPath": job.path.to_string_lossy(),
            }),
            Err(error) => {
                let mut value = json!({ "kind": "read_failed", "workId": work_id });
                if let Some(code) = error.raw_os_error() {
                    value["code"] = code.to_string().into();
                }
                value
            }
        }
    }
    pub async fn cancel(&self, session: &str, id: Option<&str>) -> Result<()> {
        let all = self.jobs.lock().await;
        if let Some(id) = id {
            all.get(session)
                .and_then(|v| v.get(id))
                .context("Background task unavailable")?
                .cancel
                .cancel();
        } else if let Some(jobs) = all.get(session) {
            for job in jobs.values() {
                job.cancel.cancel();
            }
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<()> {
        let jobs: Vec<_> = self
            .jobs
            .lock()
            .await
            .values()
            .flat_map(|jobs| jobs.values().cloned())
            .collect();
        for job in &jobs {
            job.cancel.cancel();
        }
        for job in jobs {
            let mut rx = job.state.clone();
            if rx.borrow().is_none() {
                let _ = rx.wait_for(|v| v.is_some()).await;
            }
        }
        Ok(())
    }
    pub async fn close_session(&self, session: &str) -> Result<()> {
        let jobs = self.jobs.lock().await.remove(session).unwrap_or_default();
        for job in jobs.values() {
            job.cancel.cancel();
        }
        for job in jobs.values() {
            let mut state = job.state.clone();
            if state.borrow().is_none() {
                state.wait_for(|result| result.is_some()).await?;
            }
        }
        Ok(())
    }
}
/// 首个 Bash 前向会话 owner 请求用户终端偏好（TS `resolveInitialBashShellSelection`）；
/// 无 owner（fixture/测试）、Host 不支持或超时时按自动探测处理，见 docs/specs/rust-shell-selection.md。
pub(super) async fn shell_override(
    sink: Option<&EventSink>,
) -> Option<crate::shell_select::Override> {
    let sink = sink?;
    let (reply, rx) = oneshot::channel();
    sink.send(Event::ShellPreference { reply }).await.ok()?;
    let value = tokio::time::timeout(Duration::from_millis(15_000), rx)
        .await
        .ok()?
        .ok()??;
    crate::shell_select::parse_override(&value)
}

/// 会话当前的 shell 选择（与 Bash 执行同一 Host 偏好与解析）。
async fn session_shell_selection(
    sink: &crate::contract::EventSink,
) -> crate::shell_select::Selection {
    let over = shell_override(Some(sink)).await;
    let env: Vec<(String, String)> = std::env::vars().collect();
    crate::shell_select::resolve(
        crate::shell_select::Platform::current(),
        &env,
        over.as_ref(),
        &|p| std::path::Path::new(p).is_file(),
    )
}

/// 恢复会话首轮的 shell 提醒（docs/specs/rust-shell-resume-notice.md）。
pub(super) async fn resume_notice(
    sink: &crate::contract::EventSink,
    persisted: Option<&str>,
) -> Option<String> {
    crate::shell_select::resume_notice(&session_shell_selection(sink).await, persisted)
}

/// 系统提示词里的 Shell 名（与 Bash 执行同一选择）。
pub(super) async fn display_name(sink: &crate::contract::EventSink) -> String {
    crate::shell_select::display_name(&session_shell_selection(sink).await)
}
