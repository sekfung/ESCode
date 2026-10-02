//! 工作流脚本分析子进程客户端（docs/specs/rust-dynamic-workflow.md 第 3 期，路线 1）。
//!
//! 诊断与类型结论来自 TypeScript 类型检查器，Rust 不复刻：经 Host 提供的 Node 启动器
//! （`ZCODE_PLUGIN_HOST_EXEC_PATH` + `ZCODE_PLUGIN_HOST_ENTRYPOINT`，与插件宿主同一套）运行
//! `__zcode-workflow-analyzer`，NDJSON 一问一答，结果即 TS `analyzeWorkflowScript` 的 JSON 形。
//!
//! 进程常驻（首次编译要载入 TS 与 lib，后续调用复用）、串行；超时、崩溃或协议错误即杀掉，
//! 下一次调用重新拉起。TS 对紧邻的同一脚本有单槽记忆（审批门与 handler 各分析一次），这里同样保留。

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

pub(crate) const COMMAND: &str = "__zcode-workflow-analyzer";
const TIMEOUT: Duration = Duration::from_secs(60);

struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

#[derive(Default)]
pub(crate) struct WorkflowAnalyzer {
    process: Mutex<Option<Process>>,
    last: Mutex<Option<(String, Value)>>,
}

/// Host 提供的 Node 启动器；任一缺失即视为分析器不可用（工作流写工具据此不注册）。
pub(crate) fn launcher() -> Option<(String, String)> {
    let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    Some((
        var("ZCODE_PLUGIN_HOST_EXEC_PATH")?,
        var("ZCODE_PLUGIN_HOST_ENTRYPOINT")?,
    ))
}

impl WorkflowAnalyzer {
    /// 分析一段脚本，返回 `{ok, diagnostics, declaredArtifacts, core?, graph?, causality?, flow?, handoff?}`。
    pub(crate) async fn analyze(&self, script: &str) -> Result<Value> {
        if let Some((cached, result)) = self.last.lock().await.as_ref()
            && cached == script
        {
            return Ok(result.clone());
        }
        let result = self.request("analyze", script).await?;
        *self.last.lock().await = Some((script.to_owned(), result.clone()));
        Ok(result)
    }

    /// EvalWorkflowSnippet 的确认门（TS `prepareEvalWorkflowSnippetApproval`）：编得过且带 world.run 才问。
    pub(crate) async fn snippet_gate(&self, code: &str) -> Result<bool> {
        Ok(self.request("snippetGate", code).await?["ask"] == true)
    }

    async fn request(&self, method: &str, script: &str) -> Result<Value> {
        let mut guard = self.process.lock().await;
        if guard.is_none() {
            *guard = Some(spawn()?);
        }
        let process = guard.as_mut().expect("spawned analyzer");
        process.next_id += 1;
        let id = process.next_id;
        let outcome = tokio::time::timeout(TIMEOUT, exchange(process, id, method, script)).await;
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                kill(guard.take());
                Err(error)
            }
            Err(_) => {
                kill(guard.take());
                bail!("Workflow analyzer timed out after {} s", TIMEOUT.as_secs())
            }
        }
    }
}

fn spawn() -> Result<Process> {
    let (exec, entrypoint) = launcher()
        .context("Workflow analyzer is unavailable: the Host did not provide a Node launcher")?;
    let mut command = tokio::process::Command::new(exec);
    command
        .arg(entrypoint)
        .arg(COMMAND)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    zcode_cli_host::child_env::apply(&mut command, false);
    // 桌面打包态启动器是 ZCode Helper；缺少 Node 模式会误进 Electron main。
    command.env("ELECTRON_RUN_AS_NODE", "1");
    let mut child = command
        .spawn()
        .context("Failed to start the workflow analyzer")?;
    let stdin = child.stdin.take().context("analyzer stdin")?;
    let stdout = BufReader::new(child.stdout.take().context("analyzer stdout")?).lines();
    Ok(Process {
        child,
        stdin,
        stdout,
        next_id: 0,
    })
}

fn kill(process: Option<Process>) {
    if let Some(mut process) = process {
        let _ = process.child.start_kill();
    }
}

async fn exchange(process: &mut Process, id: u64, method: &str, script: &str) -> Result<Value> {
    let line = json!({ "id": id, "method": method, "script": script }).to_string();
    process.stdin.write_all(line.as_bytes()).await?;
    process.stdin.write_all(b"\n").await?;
    process.stdin.flush().await?;
    loop {
        let line = process
            .stdout
            .next_line()
            .await?
            .ok_or_else(|| anyhow!("Workflow analyzer exited unexpectedly"))?;
        if line.trim().is_empty() {
            continue;
        }
        let reply: Value =
            serde_json::from_str(&line).context("Workflow analyzer sent an invalid reply")?;
        if reply["id"] != id {
            continue;
        }
        if let Some(error) = reply["error"].as_str() {
            bail!("Workflow analyzer failed: {error}");
        }
        return Ok(reply["result"].clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 TS 直调逐字比对（语料由 scripts/generate-zcode-cli-rust-workflow-analysis-corpus.mjs 生成）。
    /// 需要已构建的 zcode.cjs 与 node；缺少启动器时跳过（CI 由差分脚本注入）。
    #[tokio::test]
    async fn bridge_matches_ts_analyzer() {
        if launcher().is_none() {
            eprintln!("skip: ZCODE_PLUGIN_HOST_EXEC_PATH / ENTRYPOINT not set");
            return;
        }
        let corpus: Value =
            serde_json::from_str(include_str!("workflow_analysis_corpus.json")).unwrap();
        let analyzer = WorkflowAnalyzer::default();
        for case in corpus["cases"].as_array().unwrap() {
            let script = case["script"].as_str().unwrap();
            let actual = analyzer.analyze(script).await.unwrap();
            assert_eq!(actual, case["result"], "{}", case["name"]);
        }
    }
}
