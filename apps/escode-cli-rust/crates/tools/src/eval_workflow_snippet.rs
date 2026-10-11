//! `EvalWorkflowSnippet`（docs/specs/rust-dynamic-workflow.md 第 6 期）：对齐 TS
//! core/tool/handlers/eval-workflow-snippet.ts。同步编译并运行一段 scratch-facade 片段，完全瞬态。
//!
//! 生命周期：validateInput（`code` / `path` 二选一）→ resolveInput（技能门 → `path` 整份读成 `code`，
//! 不解析元数据块，路径写成绝对形）→ prepareApproval（编得过且带 world.run 命令才问，经 Node 分析器）→
//! handler：执行面是 Node 的 snippet 服务本身（`__escode-workflow-snippet` 子进程，一次调用一个进程），
//! 取消时写 `cancel` 行让 harness 收尾，宽限期后仍未退出才强杀。

use super::save_workflow::{Prepared, describe, read_script_file, skill_gate_message};
use super::workflow_analyzer::{WorkflowAnalyzer, launcher};
use crate::contract::ToolOutput;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

pub(crate) const TOOL: &str = "EvalWorkflowSnippet";
const COMMAND: &str = "__escode-workflow-snippet";
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
/// 工具级外层兜底（TS 660 s）：正常路径永远是 harness 的内层超时先到。
const OUTER_TIMEOUT: Duration = Duration::from_millis(660_000);
const CANCEL_GRACE: Duration = Duration::from_secs(5);
const SOURCE_ERROR: &str = "Provide exactly one snippet source: `code` for the snippet inline, or `path` for a file holding it. Passing both, or neither, is ambiguous.";
const NOT_EXECUTED_NOTE: &str =
    "NOTE: The snippet was NOT executed — fix the errors above and call the tool again.";
const UNAVAILABLE_NOTE: &str =
    "NOTE: The snippet was NOT executed — snippet evaluation is not available in this session.";

pub(crate) async fn prepare(
    cwd: &Path,
    args: &Value,
    skill_loaded: bool,
    analyzer: &WorkflowAnalyzer,
) -> Result<Prepared> {
    if args.get("code").is_some() == args.get("path").is_some() {
        return Ok(Err(SOURCE_ERROR.into()));
    }
    if !skill_loaded {
        return Ok(Err(skill_gate_message(TOOL)));
    }
    let mut input = args.clone();
    if let Some(path) = args["path"].as_str() {
        match read_script_file(cwd, path, false) {
            Ok((absolute, source)) => {
                input["code"] = source.into();
                input["path"] = absolute.to_string_lossy().into_owned().into();
            }
            Err(message) => return Ok(Err(message)),
        }
    }
    let code = input["code"].as_str().unwrap_or_default().to_owned();
    let ask = analyzer.snippet_gate(&code).await?;
    Ok(Ok((input, ask)))
}

fn output(
    ok: bool,
    diagnostics: Value,
    logs: Value,
    response: String,
    duration: u128,
) -> ToolOutput {
    let data = json!({
        "ok": ok,
        "diagnostics": diagnostics,
        "logs": logs,
        "response": response,
        "durationMs": duration as u64,
    });
    ToolOutput::new(response, data)
}

/// logs 渲染进 response（TS `renderLogs`）。
fn logs_section(logs: &[Value], truncated: bool) -> Vec<String> {
    if logs.is_empty() {
        return vec![];
    }
    let mut lines = vec![String::new(), "Logs:".into()];
    lines.extend(
        logs.iter()
            .map(|log| format!("- {}", log.as_str().unwrap_or_default())),
    );
    if truncated {
        lines.push("- … (logs truncated)".into());
    }
    lines
}

pub(crate) async fn execute(
    cwd: &Path,
    input: &Value,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let code = input["code"]
        .as_str()
        .context("EvalWorkflowSnippet handler received input without resolved code")?;
    let Some((exec, entrypoint)) = launcher() else {
        // 未接线的宿主：诚实降级，绝不假装执行过。
        return Ok(output(
            false,
            json!([]),
            json!([]),
            UNAVAILABLE_NOTE.into(),
            0,
        ));
    };
    let started = Instant::now();
    let request = json!({
        "code": code,
        "cwd": cwd.to_string_lossy(),
        "timeoutMs": input["timeoutMs"].as_u64().unwrap_or(DEFAULT_TIMEOUT_MS),
    });
    let mut command = tokio::process::Command::new(exec);
    command
        .arg(entrypoint)
        .arg(COMMAND)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    escode_cli_host::child_env::apply(&mut command, false);
    command.env("ELECTRON_RUN_AS_NODE", "1");
    let mut child = command
        .spawn()
        .context("Failed to start the snippet runner")?;
    let mut stdin = child.stdin.take().context("snippet runner stdin")?;
    stdin.write_all(format!("{request}\n").as_bytes()).await?;
    stdin.flush().await?;
    let mut lines = BufReader::new(child.stdout.take().context("snippet runner stdout")?).lines();
    let reply = tokio::select! {
        line = tokio::time::timeout(OUTER_TIMEOUT, lines.next_line()) => line,
        _ = cancel.cancelled() => {
            // 取消：让 harness 自己 kill 沙箱子进程并收尾；宽限期后仍未结束则强杀。
            let _ = stdin.write_all(b"cancel\n").await;
            let _ = stdin.flush().await;
            match tokio::time::timeout(CANCEL_GRACE, lines.next_line()).await {
                Ok(_) => {}
                Err(_) => {
                    let _ = child.start_kill();
                }
            }
            bail!("Cancelled");
        }
    };
    let line = match reply {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => bail!("The snippet runner exited without a result"),
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => {
            let _ = child.start_kill();
            bail!("Snippet evaluation timed out");
        }
    };
    drop(stdin);
    let _ = child.wait().await;
    let result: Value =
        serde_json::from_str(&line).context("The snippet runner sent an invalid result")?;
    let duration = started.elapsed().as_millis();
    let logs = result["logs"].as_array().cloned().unwrap_or_default();
    let truncated = result["logsTruncated"] == true;
    match result["kind"].as_str() {
        Some("diagnostics") => {
            let diagnostics = result["diagnostics"].clone();
            let described = input["path"]
                .as_str()
                .map(|path| describe(Path::new(path), cwd));
            let mut lines = vec!["The snippet has errors:".to_owned()];
            for diagnostic in diagnostics.as_array().into_iter().flatten() {
                let message = diagnostic["message"].as_str().unwrap_or_default();
                let (line, column) = (&diagnostic["line"], &diagnostic["column"]);
                lines.push(match &described {
                    Some(described) => format!("{described}:L{line}:C{column} {message}"),
                    None => format!("L{line}:C{column} {message}"),
                });
            }
            lines.push(String::new());
            lines.push(NOT_EXECUTED_NOTE.into());
            Ok(output(
                false,
                diagnostics,
                json!([]),
                lines.join("\n"),
                duration,
            ))
        }
        Some("failed") => {
            let mut lines = vec![format!(
                "The snippet failed ({}): {}",
                result["error"]["code"].as_str().unwrap_or_default(),
                result["error"]["message"].as_str().unwrap_or_default()
            )];
            lines.extend(logs_section(&logs, truncated));
            Ok(output(
                false,
                json!([]),
                Value::Array(logs),
                lines.join("\n"),
                duration,
            ))
        }
        Some("completed") => {
            let mut lines = vec![format!("The snippet completed in {duration}ms.")];
            lines.push(match result["serialized"].as_str() {
                Some(serialized) => format!("Return value:\n{serialized}"),
                None => "It returned no value.".into(),
            });
            lines.extend(logs_section(&logs, truncated));
            Ok(output(
                true,
                json!([]),
                Value::Array(logs),
                lines.join("\n"),
                duration,
            ))
        }
        _ => bail!(
            "Snippet evaluation crashed: {}",
            result["message"].as_str().unwrap_or("unknown error")
        ),
    }
}
