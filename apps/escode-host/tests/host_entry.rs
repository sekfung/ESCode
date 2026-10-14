//! Host 进程入口的端到端测试（rust-u2-host-skeleton.md §3.4/§3.5）：以真实子进程运行
//! `escode-host`，走控制通道完成 attach → 请求转发 → 帧回传 → detach → shutdown。
//!
//! 断言面：HostReady 握手、WindowAttached 携带代次、`session/list` 往返（帧经 WindowFrame 回传）、
//! 多窗口共享同一实例（同代次）、detach 后仍可服务、Shutdown 退出码 0（无残余任务）。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use escode_host_control::{ControlMessage, WorkspaceDescriptor};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Host {
    async fn start() -> Self {
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_escode-host"));
        let mut child = tokio::process::Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("host binary starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    async fn send(&mut self, message: &ControlMessage) {
        let line = serde_json::to_string(message).expect("serialize control message");
        self.stdin.write_all(line.as_bytes()).await.expect("stdin");
        self.stdin.write_all(b"\n").await.expect("stdin");
        self.stdin.flush().await.expect("flush");
    }

    async fn recv(&mut self) -> ControlMessage {
        loop {
            let mut line = String::new();
            let read =
                tokio::time::timeout(Duration::from_secs(30), self.stdout.read_line(&mut line))
                    .await
                    .expect("control frame within timeout")
                    .expect("stdout read");
            assert!(read > 0, "host closed stdout: {line:?}");
            if line.trim().is_empty() {
                continue;
            }
            return serde_json::from_str(&line).unwrap_or_else(|error| {
                panic!("invalid control frame {line:?}: {error}");
            });
        }
    }

    /// 读帧直到谓词命中（丢弃其它帧）。
    async fn recv_until(
        &mut self,
        what: &str,
        predicate: impl Fn(&ControlMessage) -> bool,
    ) -> ControlMessage {
        for _ in 0..200 {
            let message = self.recv().await;
            if predicate(&message) {
                return message;
            }
        }
        panic!("no frame matched {what}");
    }

    async fn shutdown(mut self) -> i32 {
        self.send(&ControlMessage::Shutdown {
            reason: "test".to_owned(),
        })
        .await;
        // 关闭 stdin，Host 排空后退出。
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(60), self.child.wait())
            .await
            .expect("host exits")
            .expect("wait");
        status.code().unwrap_or(-1)
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    data_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let workspace = root.path().join("workspace");
        let data_dir = root.path().join("data");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&data_dir).expect("data dir");
        Self {
            _root: root,
            workspace,
            data_dir,
        }
    }

    fn descriptor(&self) -> WorkspaceDescriptor {
        WorkspaceDescriptor {
            path: self.workspace.to_string_lossy().into_owned(),
            identity: Some(self.workspace.to_string_lossy().into_owned()),
            data_dir: Some(self.data_dir.to_string_lossy().into_owned()),
            config: None,
        }
    }

    fn env(&self) -> BTreeMap<String, String> {
        BTreeMap::from([("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned())])
    }
}

fn attach(window: &str, workspace: WorkspaceDescriptor) -> ControlMessage {
    ControlMessage::WindowAttach {
        window_id: window.to_owned(),
        client_id: "test-client".to_owned(),
        workspace,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_attaches_relays_frames_and_shuts_down_cleanly() {
    let fixture = Fixture::new();
    let mut host = Host::start().await;
    let _ = fixture.env();

    // 1. 握手。
    let ready = host.recv().await;
    assert!(
        matches!(
            ready,
            ControlMessage::HostReady {
                protocol_version: 1,
                ..
            }
        ),
        "首帧必须是 HostReady: {ready:?}"
    );

    // 2. 接入窗口 → Host 为该 workspace 建实例。
    host.send(&attach("win-1", fixture.descriptor())).await;
    let attached = host
        .recv_until("WindowAttached", |message| {
            matches!(message, ControlMessage::WindowAttached { .. })
        })
        .await;
    let ControlMessage::WindowAttached {
        window_id,
        generation,
    } = attached
    else {
        unreachable!()
    };
    assert_eq!(window_id, "win-1");
    assert_eq!(generation, 1, "首个实例是第一代");

    // 3. 请求转发：session/list 经实例往返，应答以 WindowFrame 回传。
    host.send(&ControlMessage::WindowRequest {
        window_id: "win-1".to_owned(),
        line: json!({"id":"list-1","method":"session/list","params":{}}).to_string(),
    })
    .await;
    let reply = host
        .recv_until("session/list reply", |message| {
            matches!(message, ControlMessage::WindowFrame { line, .. } if line.contains("list-1"))
        })
        .await;
    let ControlMessage::WindowFrame { window_id, line } = reply else {
        unreachable!()
    };
    assert_eq!(window_id, "win-1");
    let frame: Value = serde_json::from_str(&line).expect("NDJSON");
    assert_eq!(frame["id"], "list-1");
    assert_eq!(frame["result"]["sessions"], json!([]));

    // 4. 第二个窗口接入同一 workspace：复用同一实例（代次不变）。
    host.send(&attach("win-2", fixture.descriptor())).await;
    let second = host
        .recv_until("second WindowAttached", |message| {
            matches!(message, ControlMessage::WindowAttached { window_id, .. } if window_id == "win-2")
        })
        .await;
    let ControlMessage::WindowAttached { generation, .. } = second else {
        unreachable!()
    };
    assert_eq!(generation, 1, "同一 workspace 的第二个窗口复用第一代实例");

    // 5. 断开一个窗口后另一个仍可服务（帧泵挂断窗口不影响实例）。
    host.send(&ControlMessage::WindowDetach {
        window_id: "win-2".to_owned(),
    })
    .await;
    let _ = host
        .recv_until("WindowDetached", |message| {
            matches!(message, ControlMessage::WindowDetached { window_id } if window_id == "win-2")
        })
        .await;
    host.send(&ControlMessage::WindowRequest {
        window_id: "win-1".to_owned(),
        line: json!({"id":"list-2","method":"session/list","params":{}}).to_string(),
    })
    .await;
    let _ = host
        .recv_until("second reply", |message| {
            matches!(message, ControlMessage::WindowFrame { line, .. } if line.contains("list-2"))
        })
        .await;

    // 6. 收口：退出码 0（无残余任务/进程）。
    assert_eq!(host.shutdown().await, 0, "干净退出");
}

#[tokio::test(flavor = "multi_thread")]
async fn unattached_window_requests_are_rejected_with_structured_error() {
    let mut host = Host::start().await;
    let ready = host.recv().await;
    assert!(matches!(ready, ControlMessage::HostReady { .. }));
    host.send(&ControlMessage::WindowRequest {
        window_id: "ghost".to_owned(),
        line: json!({"id":"x","method":"session/list","params":{}}).to_string(),
    })
    .await;
    let failed = host
        .recv_until("Failed", |message| {
            matches!(message, ControlMessage::Failed { .. })
        })
        .await;
    let ControlMessage::Failed { error } = failed else {
        unreachable!()
    };
    assert!(
        error.message.contains("not attached"),
        "结构化错误点名窗口: {error:?}"
    );
    assert_eq!(host.shutdown().await, 0);
}
