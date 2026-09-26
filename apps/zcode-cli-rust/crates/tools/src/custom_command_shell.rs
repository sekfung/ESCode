//! 自定义命令解析入口与 shell 展开执行（TS `custom-command-prompt.ts`、`custom-command-shell-expansion.ts`）。
//! shell 取自动探测结果（admission 阶段没有 run sink 可询问终端偏好），见 docs/specs/rust-custom-commands.md。
use super::custom_commands::{self as commands, Command};
use crate::domain::custom_command::{self as cc, Segment};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// 协议 `slashCommands` 目录：内置段 + 可在 App 中执行的自定义命令；发现失败时只保留内置段。
pub(crate) async fn catalog(cwd: &Path, cancel: &CancellationToken) -> Vec<Value> {
    let mut entries = cc::builtin_catalog();
    if let Ok(found) = commands::discover(cwd, cancel).await {
        entries.extend(
            found
                .iter()
                .filter(|c| !c.meta.disable_non_interactive && !cc::is_reserved(&c.meta.name))
                .map(|c| c.meta.catalog_entry()),
        );
    }
    entries
}

/// `/name args` 展开成提示词；不是命令、保留名或不存在时返回 None（按原文发送）。
pub(crate) async fn resolve(
    cwd: &Path,
    session: Option<&str>,
    text: &str,
    cancel: &CancellationToken,
) -> Result<Option<String>> {
    let Some((name, args)) = cc::parse_invocation(text) else {
        return Ok(None);
    };
    if cc::is_reserved(&name) {
        return Ok(None);
    }
    let name = cc::normalize_name(&name);
    let Some(command) = commands::discover(cwd, cancel)
        .await?
        .into_iter()
        .find(|c| c.meta.name == name)
    else {
        return Ok(None);
    };
    let content = commands::content(&command).await?;
    let expansion = cc::expand_template(&content, &args);
    let body = expand_shell(&command, &expansion.body, cwd, session, cancel).await?;
    let meta = &command.meta;
    Ok(Some(cc::format_prompt(
        &meta.name,
        &meta.scope,
        &meta.source,
        &meta.skills,
        &body,
    )))
}

async fn expand_shell(
    command: &Command,
    content: &str,
    cwd: &Path,
    session: Option<&str>,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut output = String::new();
    for segment in cc::shell_segments(content) {
        match segment {
            Segment::Text(text) => output.push_str(&text),
            Segment::Shell(shell) if shell.is_empty() => {}
            Segment::Shell(shell) => {
                let plugin = command.plugin.clone().or_else(|| {
                    cc::infer_plugin(&command.meta.source, &command.root.to_string_lossy())
                });
                cc::check_shell_context(
                    &command.meta.name,
                    &shell,
                    session.is_some(),
                    plugin.is_some(),
                )
                .map_err(|e| anyhow!(e))?;
                let cwd_text = cwd.to_string_lossy();
                let env = cc::shell_env(&cwd_text, session, plugin.as_ref());
                let run = run(cwd, &shell, &env, cancel).await?;
                if run.status == "completed" && run.code.unwrap_or(0) == 0 {
                    output.push_str(run.stdout.trim_end_matches(cc::js_ws));
                } else {
                    // TS ExecutionResult：正常结束但退出码非零时 status 为 failed。
                    let status = if run.status == "completed" {
                        "failed"
                    } else {
                        run.status
                    };
                    let exit = run
                        .code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| status.to_owned());
                    anyhow::bail!(cc::shell_failure(
                        &command.meta.name,
                        &shell,
                        &exit,
                        None,
                        &run.stderr,
                        &run.stdout,
                        status,
                    ));
                }
            }
        }
    }
    Ok(output)
}

struct Run {
    status: &'static str,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}
async fn read_limited<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> String {
    let mut kept = vec![];
    let mut chunk = [0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let room = cc::SHELL_OUTPUT_BYTES.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..n.min(room)]);
    }
    String::from_utf8_lossy(&kept).into_owned()
}
async fn run(
    cwd: &Path,
    shell: &str,
    extra: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<Run> {
    let platform = crate::shell_select::Platform::current();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let selection =
        crate::shell_select::resolve(platform, &env, None, &|p| std::path::Path::new(p).is_file());
    let plan = crate::shell_select::spawn_plan(platform, &env, &selection, shell);
    let mut process = tokio::process::Command::new(&plan.file);
    zcode_cli_host::child_env::apply(&mut process, true);
    process
        .args(&plan.args)
        .envs(plan.env_overlay)
        .envs(extra.iter().cloned())
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process.spawn().context("Cannot start shell")?;
    let pid = child.id().context("Missing child pid")?;
    #[cfg(windows)]
    let job = super::win_job::Job::attach(pid);
    let out = tokio::spawn(read_limited(child.stdout.take().unwrap()));
    let err = tokio::spawn(read_limited(child.stderr.take().unwrap()));
    let (status, code) = tokio::select! {biased;
        _=cancel.cancelled()=>("cancelled",None),
        _=tokio::time::sleep(Duration::from_millis(cc::SHELL_TIMEOUT_MS))=>("timed_out",None),
        result=child.wait()=>("completed",result?.code()),
    };
    if status != "completed" {
        #[cfg(windows)]
        if let Some(job) = &job {
            job.terminate();
        }
        super::tool_process::terminate(&mut child, pid, false).await?;
    }
    let collect = async { (out.await, err.await) };
    let (stdout, stderr) = tokio::time::timeout(Duration::from_secs(1), collect)
        .await
        .unwrap_or((Ok(String::new()), Ok(String::new())));
    Ok(Run {
        status,
        code,
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
    })
}
