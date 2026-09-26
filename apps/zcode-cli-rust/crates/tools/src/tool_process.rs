use super::tools::check_cancel;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
pub(super) const INLINE: usize = 24 * 1024;
/// TS Bash `MAX_INLINE_OUTPUT_BYTES`：结果 stdout 取合并输出文件的前这么多字节。
const MODEL_INLINE: u64 = 30_000;
const MAX_STREAM: u64 = 16 * 1024 * 1024;
/// 两路输出共用一个 OS 管道（TS `BashFileOutput` 让 stdout/stderr 共用同一个 fd）：内核按写入顺序交付，
/// 合并文件中的交错顺序与进程实际输出一致。之前分别读两条管道，Linux CI 上 stderr 先于 stdout 落盘。
/// 读取在阻塞线程中进行：Windows 匿名管道不支持异步读。
fn capture(
    mut reader: std::io::PipeReader,
    mut file: std::fs::File,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut copy = || -> Result<()> {
            let mut bytes = 0u64;
            let mut buf = [0u8; 8192];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                let write = n.min(MAX_STREAM.saturating_sub(bytes) as usize);
                if write > 0 {
                    file.write_all(&buf[..write])?;
                }
                bytes += n as u64;
                if bytes > MAX_STREAM {
                    cancel.cancel();
                    break;
                }
            }
            Ok(())
        };
        let result = copy();
        // 输出落盘失败必须停止进程树，不能让已丢失输出的后台任务继续运行。
        if result.is_err() {
            cancel.cancel();
        }
        result
    })
}
pub(super) async fn run(
    cwd: &Path,
    text: &str,
    path: &Path,
    combined: Arc<Mutex<tokio::fs::File>>,
    timeout: Option<Duration>,
    cancel: &CancellationToken,
    shell: ShellContext<'_>,
) -> Result<Value> {
    check_cancel(cancel)?;
    let command_text = text;
    // 原实现在 Windows 固定 cmd.exe、POSIX 固定 /bin/bash，与 TS 自动选择 Git Bash / $SHELL 的语义不一致；
    // 改为复用 shell_select（对应 TS bash-shell-provider），见 docs/specs/rust-shell-selection.md。
    let platform = crate::shell_select::Platform::current();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let selection = crate::shell_select::resolve(platform, &env, shell.over, &|p| {
        std::path::Path::new(p).is_file()
    });
    // 与 TS 默认 embedded search 分支一致：posix / git-bash 会话在命令前 source find/grep prelude
    // （模型工具面不再暴露 Glob/Grep，见 docs/specs/rust-tool-surface.md）；cmd / legacy 不注入。
    let dialect = match selection.dialect {
        crate::shell_select::Dialect::Posix => "posix",
        crate::shell_select::Dialect::GitBash => "git-bash",
        crate::shell_select::Dialect::Cmd => "cmd",
        crate::shell_select::Dialect::Legacy => "legacy-shell",
    };
    let text =
        match crate::embedded_search::prelude(&crate::embedded_search::backend(&env), dialect) {
            Some(content) => {
                crate::embedded_search::prepend_source(
                    text,
                    &content,
                    shell.startup_root,
                    shell.session,
                    dialect == "git-bash",
                )
                .await?
            }
            None => text.to_owned(),
        };
    let plan = crate::shell_select::spawn_plan(platform, &env, &selection, &text);
    let (reader, writer) = std::io::pipe()?;
    let file = combined.lock().await.try_clone().await?.into_std().await;
    let mut command = Command::new(&plan.file);
    // 先清洗运行时环境并恢复出网配置（TS execution-command），再叠加 shell 计划自身的变量。
    zcode_cli_host::child_env::apply(&mut command, true);
    command.args(&plan.args).envs(plan.env_overlay);
    command
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("Cannot start shell")?;
    // 父进程持有的写端必须随 Command 释放，否则管道永远等不到 EOF。
    drop(command);
    let pid = child.id().context("Missing child pid")?;
    // Windows 上把 shell 放进 Job，终止时连同 MSYS 后代一起回收；附加失败时退回 taskkill。
    #[cfg(windows)]
    let job = super::win_job::Job::attach(pid);
    let overflow = CancellationToken::new();
    let mut out = capture(reader, file, overflow.clone());
    let timer = async {
        if let Some(t) = timeout {
            tokio::time::sleep(t).await
        } else {
            std::future::pending::<()>().await
        }
    };
    tokio::pin!(timer);
    let (status, mut reason) = tokio::select! {biased;
        _=cancel.cancelled()=>(None,"cancelled"),
        _=overflow.cancelled()=>(None,"failed"),
        _=&mut timer=>(None,"timed_out"),
        result=child.wait()=>(Some(result?),"completed"),
    };
    let captured = async {
        kill_tree(
            &mut child,
            pid,
            status.is_none(),
            #[cfg(windows)]
            job.as_ref(),
        )
        .await?;
        let mut streams = &mut out;
        if reason != "completed" {
            // 首次 wait 就被取消时也必须验证管道关闭，不能只在 leader 已退出的分支限时。
            return tokio::time::timeout(Duration::from_secs(1), &mut streams)
                .await
                .context("Bash descendants still hold output after termination");
        }
        // leader exit 不是整个 IO 生命周期结束；仍持有管道的后代不能绕过取消和超时。
        let ready = tokio::select! {biased;
            _=cancel.cancelled(), if reason=="completed"=>{reason="cancelled"; None},
            _=overflow.cancelled(), if reason=="completed"=>{reason="failed"; None},
            _=&mut timer, if reason=="completed"=>{reason="timed_out"; None},
            result=&mut streams=>Some(result),
        };
        if let Some(result) = ready {
            return Ok(result);
        }
        kill_tree(
            &mut child,
            pid,
            true,
            #[cfg(windows)]
            job.as_ref(),
        )
        .await?;
        // 期限只用于报告无法确认回收的错误，绝不把未关闭的 pipe 当作成功。
        tokio::time::timeout(Duration::from_secs(1), &mut streams)
            .await
            .context("Bash descendants still hold output after termination")
    }
    .await
    .context(crate::contract::ProcessCleanupFailure);
    if captured.is_err() {
        out.abort();
        return Err(captured.err().unwrap());
    }
    captured.unwrap()??;
    let size = {
        let mut file = combined.lock().await;
        file.flush().await?;
        file.metadata().await?.len()
    };
    let mut head = vec![];
    tokio::fs::File::open(path)
        .await?
        .take(MODEL_INLINE)
        .read_to_end(&mut head)
        .await?;
    let reason = if reason == "completed" && !status.as_ref().unwrap().success() {
        "failed"
    } else {
        reason
    };
    let exit_code = status.and_then(|s| s.code());
    let truncated = size > head.len() as u64;
    // TS BashFileOutput：两路输出共用一个文件，stdout 为文件开头、stderr 为空（docs/specs/rust-bash-model-content.md）。
    let mut data = json!({"stdout":String::from_utf8_lossy(&head),"stderr":"","interrupted":reason=="cancelled"||reason=="timed_out","isImage":false,"noOutputExpected":crate::domain::bash_model_content::is_silent(command_text),"status":reason,"timedOut":reason=="timed_out","cancelled":reason=="cancelled","stdoutTruncated":truncated,"stderrTruncated":false,"stdoutBytes":size,"stderrBytes":0});
    if let Some(code) = exit_code {
        data["exitCode"] = code.into();
    }
    let rules = crate::domain::bash_model_content::stop_message;
    if let Some(message) = rules(reason, timeout.map(|t| t.as_millis() as u64), overflow.is_cancelled()) {
        data["stderr"] = message.into();
    }
    let interpretation = if overflow.is_cancelled() {
        Some("Command stopped because output exceeded the configured limit".to_owned())
    } else {
        crate::domain::bash_model_content::return_code_interpretation(command_text, reason, exit_code.map(i64::from))
    };
    if let Some(interpretation) = interpretation {
        data["returnCodeInterpretation"] = interpretation.into();
    }
    // 前台只在截断时保留输出文件（TS persistOutput: on_truncate）；后台任务始终保留，TaskOutput 从中读取。
    let backgrounded = shell
        .lifecycle
        .compare_exchange(FOREGROUND, SETTLED, Ordering::SeqCst, Ordering::SeqCst)
        .is_err();
    if truncated || backgrounded {
        let path = path.to_string_lossy();
        for key in ["rawOutputPath", "persistedOutputPath", "stdoutPersistedOutputPath"] {
            data[key] = path.as_ref().into();
        }
        data["persistedOutputSize"] = size.into();
        data["stdoutPersistedOutputSize"] = size.into();
    } else {
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(data)
}

async fn kill_tree(
    child: &mut tokio::process::Child,
    pid: u32,
    graceful: bool,
    #[cfg(windows)] job: Option<&super::win_job::Job>,
) -> Result<()> {
    #[cfg(windows)]
    if let Some(job) = job {
        job.terminate();
    }
    terminate(child, pid, graceful).await
}
pub(super) async fn terminate(
    child: &mut tokio::process::Child,
    pid: u32,
    graceful: bool,
) -> Result<()> {
    #[cfg(unix)]
    {
        super::process_tree::terminate(child, pid, graceful).await
    }
    #[cfg(windows)]
    {
        let _ = graceful;
        let result = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await;
        if child.try_wait()?.is_none() {
            anyhow::ensure!(result?.success(), "Bash taskkill failed");
            child.wait().await?;
        }
        Ok(())
    }
}

/// Bash 执行的会话上下文：用户选择的 shell，以及 prelude 落盘位置（会话维度）。
#[derive(Clone, Copy)]
pub(super) struct ShellContext<'a> {
    pub over: Option<&'a crate::shell_select::Override>,
    pub startup_root: &'a Path,
    pub session: &'a str,
    /// 前台/后台归属（docs/specs/rust-bash-auto-background.md）：进程结束时由 FOREGROUND 原子地
    /// 结算为 SETTLED；超时转后台时由 FOREGROUND 改为 BACKGROUNDED。两者只有一方成功。
    pub lifecycle: &'a AtomicU8,
}
pub(super) const FOREGROUND: u8 = 0;
pub(super) const BACKGROUNDED: u8 = 1;
pub(super) const SETTLED: u8 = 2;

pub(super) fn shell_output(data: Value) -> crate::contract::ToolOutput {
    let failed = matches!(
        data["status"].as_str(),
        Some("failed" | "timed_out" | "cancelled" | "spawn_error")
    );
    // 模型可见正文按 TS formatBashModelContent 格式化；结构化结果留给行投影。
    let content = crate::domain::bash_model_content::format(&data);
    let mut output = crate::contract::ToolOutput::new(content, data);
    output.failed = failed;
    output
}
