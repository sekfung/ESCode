//! svn 命令执行（TS `providers/svnCommandProvider.ts`）。
//!
//! 与 Git 不同，svn 需要把密码经 stdin 传入（`--password-from-stdin`），因此这里保留
//! 可选的 stdin。stdout/stderr 合并计量输出上限；超限标记 `output_truncated`，
//! 上层按「输出被截断仍算成功」处理（与 TS 一致）。

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::config::{DISCOVERY_TIMEOUT, parse_version};

pub struct RunOptions<'a> {
    pub cwd: &'a str,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// `None` = 被信号终止或超时。
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_truncated: bool,
    pub duration_ms: u64,
}

/// TS `readSvnVersion`：探测候选二进制并返回版本号。
pub async fn probe_version(candidate: &str, env: &BTreeMap<String, String>) -> Option<String> {
    let mut command = Command::new(candidate);
    command
        .args(["--version", "--quiet"])
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(DISCOVERY_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// 执行一次 svn 命令。spawn 失败不抛错，回 `exit_code = -2` 与错误文案。
pub async fn run(binary: &str, env: &BTreeMap<String, String>, options: RunOptions<'_>) -> Output {
    let started = Instant::now();
    let has_stdin = options.stdin.is_some();
    let mut command = Command::new(binary);
    command
        .args(&options.args)
        .current_dir(options.cwd)
        .env_clear()
        .envs(env)
        .stdin(if has_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Output {
                stderr: error.to_string(),
                exit_code: Some(-2),
                duration_ms: elapsed_ms(started),
                ..Output::default()
            };
        }
    };
    if let (Some(input), Some(mut stdin)) = (options.stdin, child.stdin.take()) {
        let _ = stdin.write_all(input.as_bytes()).await;
        // 关闭 stdin，svn 才会结束密码读取。
        drop(stdin);
    }
    let waited = tokio::time::timeout(options.timeout, child.wait_with_output()).await;
    let mut result = match waited {
        Ok(Ok(output)) => Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
            timed_out: false,
            output_truncated: false,
            duration_ms: elapsed_ms(started),
        },
        Ok(Err(error)) => Output {
            stderr: error.to_string(),
            exit_code: Some(-2),
            duration_ms: elapsed_ms(started),
            ..Output::default()
        },
        Err(_) => Output {
            exit_code: None,
            timed_out: true,
            duration_ms: elapsed_ms(started),
            ..Output::default()
        },
    };
    // TS 按 stdout/stderr 合计字节数计量；超限后 kill 并保留已收内容。
    let total = result.stdout.len() + result.stderr.len();
    if total > options.max_output_bytes {
        result.output_truncated = true;
        let room = options.max_output_bytes.saturating_sub(result.stderr.len());
        truncate_utf8(&mut result.stdout, room);
    }
    result
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
