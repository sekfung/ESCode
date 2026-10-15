//! git 命令执行（TS `providers/gitCommandProvider.ts` + `gitEnvironmentProvider.ts`）。
//!
//! 语义（docs/specs/rust-u2-git.md §3）：stdout/stderr **各自**计字节，任一超限即 kill 并标记
//! `output_truncated`（超限那一块不入缓冲）；到点超时 kill；spawn 失败不抛，回 `exit_code = -2`、
//! `stderr = 错误文案`（上层按失败处理）。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// TS `DEFAULT_GIT_COMMAND_TIMEOUT_MS` 等常量。
pub const COMMAND_TIMEOUT: Duration = Duration::from_millis(15_000);
pub const DIFF_TIMEOUT: Duration = Duration::from_millis(20_000);
pub const PUSH_TIMEOUT: Duration = Duration::from_millis(10 * 60_000);
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(3_000);
pub const OUTPUT_BYTES: usize = 512 * 1024;
pub const DIFF_BYTES: usize = 1024 * 1024;
pub const PUSH_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// TS `DEFAULT_TIMEOUT_KILL_GRACE_MS`：kill 后等待进程退出的宽限。
const KILL_GRACE: Duration = Duration::from_millis(2_000);

/// TS `GIT_LOCAL_ENV_VARS`：pre-push hook 注入的仓库本地变量，必须剔除（否则命令串仓）。
const GIT_LOCAL_ENV_VARS: [&str; 16] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// TS `getGitCommandEnv`：进程 env 剔除本地变量后叠加 ESCode 约束。
pub fn command_env(process_env: &[(String, String)]) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = process_env
        .iter()
        .filter(|(key, _)| !GIT_LOCAL_ENV_VARS.contains(&key.as_str()))
        .cloned()
        .collect();
    for (key, value) in [
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_PAGER", "cat"),
        ("PAGER", "cat"),
        ("TERM", "dumb"),
        ("LC_ALL", "C"),
        ("LANG", "C"),
    ] {
        env.insert(key.to_owned(), value.to_owned());
    }
    env
}

/// TS `getGitBinaryCandidates`。
pub fn binary_candidates(process_env: &[(String, String)]) -> Vec<String> {
    let get = |key: &str| {
        process_env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    let mut candidates: Vec<String> = Vec::new();
    if let Some(explicit) = get("ESCODE_GIT_BINARY")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        candidates.push(explicit);
    }
    candidates.push("git".to_owned());
    if cfg!(windows) {
        let program_files = get("ProgramW6432")
            .or_else(|| get("ProgramFiles"))
            .unwrap_or_else(|| "C:\\Program Files".to_owned());
        let program_files_x86 =
            get("ProgramFiles(x86)").unwrap_or_else(|| "C:\\Program Files (x86)".to_owned());
        for base in [
            &program_files,
            &program_files,
            &program_files_x86,
            &program_files_x86,
        ]
        .iter()
        .zip(["cmd", "bin", "cmd", "bin"])
        {
            candidates.push(format!("{}\\Git\\{}\\git.exe", base.0, base.1));
        }
    }
    candidates
}

/// 一次 git 调用。
pub struct GitRun<'a> {
    pub cwd: &'a str,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    /// 叠加在命令 env 之上（TS `command.env`，如临时 `GIT_INDEX_FILE`）。
    pub extra_env: Vec<(String, String)>,
}

impl<'a> GitRun<'a> {
    pub fn new(cwd: &'a str, args: &[&str]) -> Self {
        Self {
            cwd,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            timeout: COMMAND_TIMEOUT,
            max_output_bytes: OUTPUT_BYTES,
            extra_env: Vec::new(),
        }
    }

    pub fn args(cwd: &'a str, args: Vec<String>) -> Self {
        Self {
            cwd,
            args,
            timeout: COMMAND_TIMEOUT,
            max_output_bytes: OUTPUT_BYTES,
            extra_env: Vec::new(),
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.extra_env.push((key.to_owned(), value.to_owned()));
        self
    }
}

/// TS `GitCommandExecutionResult` 中上层用到的字段。
#[derive(Clone, Debug, Default)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    /// `None` = 被信号终止（TS `exitCode: null`）。
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_truncated: bool,
    pub duration_ms: u64,
    pub timeout_ms: u64,
    pub timeout_elapsed_ms: Option<u64>,
}

/// 执行一次 git 命令。
pub async fn run(binary: &str, env: &BTreeMap<String, String>, run: GitRun<'_>) -> GitOutput {
    let started = Instant::now();
    let timeout_ms = u64::try_from(run.timeout.as_millis()).unwrap_or(u64::MAX);
    let mut command = Command::new(binary);
    command
        .args(&run.args)
        .current_dir(run.cwd)
        .env_clear()
        .envs(env)
        .envs(run.extra_env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return GitOutput {
                stderr: error.to_string(),
                exit_code: Some(-2),
                duration_ms: elapsed(started),
                timeout_ms,
                ..GitOutput::default()
            };
        }
    };
    let truncated = Arc::new(AtomicBool::new(false));
    let truncated_notify = Arc::new(tokio::sync::Notify::new());
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let reader = |pipe, sink| {
        tokio::spawn(collect(
            pipe,
            sink,
            run.max_output_bytes,
            truncated.clone(),
            truncated_notify.clone(),
        ))
    };
    let mut stdout_task = reader(child.stdout.take().map(Pipe::Out), stdout.clone());
    let mut stderr_task = reader(child.stderr.take().map(Pipe::Err), stderr.clone());

    // TS 在 `close`（进程退出且两路管道关闭）时才结算；超时后 kill，宽限期内仍未 close 则判 orphaned，
    // 放弃管道（hook 的孙进程可能长期持有管道，不能无界等待读端）。
    let (mut status, mut stdout_done, mut stderr_done) = (None, false, false);
    let mut timed_out = false;
    let mut timeout_elapsed_ms = None;
    let deadline = tokio::time::sleep(run.timeout);
    tokio::pin!(deadline);
    loop {
        if status.is_some() && stdout_done && stderr_done {
            break;
        }
        tokio::select! {
            exit = child.wait(), if status.is_none() => status = Some(exit.ok()),
            _ = &mut stdout_task, if !stdout_done => stdout_done = true,
            _ = &mut stderr_task, if !stderr_done => stderr_done = true,
            () = truncated_notify.notified() => {
                let _ = child.start_kill();
            }
            () = &mut deadline => {
                if timed_out {
                    stdout_task.abort();
                    stderr_task.abort();
                    break;
                }
                timed_out = true;
                timeout_elapsed_ms = Some(elapsed(started));
                let _ = child.start_kill();
                deadline.as_mut().reset(tokio::time::Instant::now() + KILL_GRACE * 2);
            }
        }
    }
    let orphaned = !(status.is_some() && stdout_done && stderr_done);
    let take = |sink: &Arc<Mutex<Vec<u8>>>| {
        String::from_utf8_lossy(
            &sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    };
    GitOutput {
        stdout: take(&stdout),
        stderr: take(&stderr),
        exit_code: if orphaned {
            None
        } else {
            status.flatten().and_then(|status| status.code())
        },
        timed_out,
        output_truncated: truncated.load(Ordering::SeqCst),
        duration_ms: elapsed(started),
        timeout_ms,
        timeout_elapsed_ms,
    }
}

enum Pipe {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

/// 读一路管道：单路累计超过上限即置截断并通知 kill；截断后两路都停止收集（TS `if (outputTruncated) return;`），
/// 但继续排空直到 EOF。
async fn collect(
    pipe: Option<Pipe>,
    sink: Arc<Mutex<Vec<u8>>>,
    limit: usize,
    truncated: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
) {
    let Some(mut pipe) = pipe else { return };
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut collected = 0_usize;
    loop {
        let read = match &mut pipe {
            Pipe::Out(pipe) => pipe.read(&mut chunk).await,
            Pipe::Err(pipe) => pipe.read(&mut chunk).await,
        };
        let count = match read {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        if truncated.load(Ordering::SeqCst) {
            continue;
        }
        if collected + count > limit {
            truncated.store(true, Ordering::SeqCst);
            notify.notify_one();
            continue;
        }
        collected += count;
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(&chunk[..count]);
    }
}

/// TS `canExecuteGitCandidate`：`--version` 3 s 内成功即可用。
pub async fn can_execute(candidate: &str, env: &BTreeMap<String, String>) -> bool {
    let mut command = Command::new(candidate);
    command
        .arg("--version")
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match tokio::time::timeout(DISCOVERY_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// TS `toResultMessage`。
pub fn result_message(output: &GitOutput) -> String {
    let stderr = crate::git::paths::js_trim(&output.stderr);
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let stdout = crate::git::paths::js_trim(&output.stdout);
    if !stdout.is_empty() {
        return stdout.to_owned();
    }
    format!(
        "exitCode={}",
        output
            .exit_code
            .map_or_else(|| "null".to_owned(), |code| code.to_string())
    )
}

/// TS `ensureGitCommandSucceeded`。
pub fn ensure_succeeded(
    label: &str,
    output: &GitOutput,
    allowed_exit_codes: &[i32],
) -> anyhow::Result<()> {
    if output.timed_out {
        let mut details = vec![format!("elapsed={}ms", output.duration_ms)];
        if let Some(kill_at) = output.timeout_elapsed_ms {
            details.push(format!("killAt={kill_at}ms"));
            details.push(format!(
                "cleanup={}ms",
                output.duration_ms.saturating_sub(kill_at)
            ));
        }
        anyhow::bail!(
            "{label} timed out after {}ms ({})",
            output.timeout_ms,
            details.join(", ")
        );
    }
    if output.output_truncated {
        anyhow::bail!("{label} output exceeded limit");
    }
    if output
        .exit_code
        .is_some_and(|code| allowed_exit_codes.contains(&code))
    {
        return Ok(());
    }
    anyhow::bail!("{label} failed: {}", result_message(output))
}
