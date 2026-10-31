// 这些用例用本机 `sh` 模拟远端 POSIX 目标；Windows 没有等价 shell，主链由桌面 E2E 覆盖（S4-i）。
#![cfg(unix)]

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Child;

use super::{CommandOutput, RemoteRunner, read_stdout_line, run_command, spawn_piped_command};
use crate::source_server_transport::TransportCommand;

struct LocalRunner;

#[async_trait]
impl RemoteRunner for LocalRunner {
    async fn run(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> anyhow::Result<CommandOutput> {
        run_command(&local_command(script), stdin, timeout).await
    }

    fn spawn_piped(&self, script: &str) -> anyhow::Result<Child> {
        spawn_piped_command(&local_command(script))
    }
}

fn local_command(script: &str) -> TransportCommand {
    TransportCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: BTreeMap::new(),
    }
}

#[tokio::test]
async fn runs_scripts_and_forwards_stdin() {
    let output = LocalRunner
        .run(
            "read line; printf 'got:%s\\n' \"$line\"",
            Some(b"hello\n"),
            Duration::from_secs(5),
        )
        .await
        .expect("run");
    assert_eq!(output.stdout.trim(), "got:hello");
    assert_eq!(output.exit_code, Some(0));
}

#[tokio::test]
async fn surfaces_stderr_and_nonzero_exit() {
    let error = LocalRunner
        .run("echo boom >&2; exit 7", None, Duration::from_secs(5))
        .await
        .expect_err("failure");
    let message = error.to_string();
    assert!(message.contains("exit 7"), "{message}");
    assert!(message.contains("boom"), "{message}");
}

#[tokio::test]
async fn times_out_long_running_scripts() {
    let error = LocalRunner
        .run("sleep 5", None, Duration::from_millis(100))
        .await
        .expect_err("timeout");
    assert!(error.to_string().contains("timed out"), "{error}");
}

#[tokio::test]
async fn reads_markers_from_piped_stdout() {
    let mut child = LocalRunner
        .spawn_piped("printf 'one\\ntwo\\n'")
        .expect("spawn");
    let first = read_stdout_line(&mut child, Duration::from_secs(5))
        .await
        .expect("first");
    assert_eq!(first.as_deref(), Some("one\n"));
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[tokio::test]
async fn handles_empty_stdout() {
    let mut command = local_command("true");
    command.program = "sh".into();
    // spawn_piped_command 总会接管 stdout；这里验证 child 被正确创建。
    let mut child = spawn_piped_command(&command).expect("spawn");
    let line = read_stdout_line(&mut child, Duration::from_secs(5))
        .await
        .expect("line");
    assert!(line.is_none());
    let _ = child.wait().await;
}
