//! runtime 实例崩溃隔离（docs/specs/rust-u2-host-resilience.md §2，验收 H2.7）。
//!
//! 两个 workspace A、B 各有一个进行中的请求（`workspace/generateText`，模型桩收到请求后挂住不答，
//! 因此请求确定地停在 runtime 内）和一个订阅。然后让 A 崩溃：
//! - embedded：发 `test/panic`（`fault-injection` feature，只在测试构建编译），engine 循环 panic；
//! - child：对 A 的 CLI 子进程 SIGKILL（Windows 用 `taskkill /F`，即 TerminateProcess）。
//!
//! 断言：A 的在飞请求以 Node 形状的 `RuntimeClosed` 失败、订阅结束且不是 lagged、生命周期只发
//! A 的 `unavailable`、崩溃回收后残余为空；B 的在飞请求与后续请求照常；A 下一次 `get` 懒重启为第 2 代。
//! 全部用通道/事件等待状态，不用 sleep；超时只作失败保护。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use escode_cli_rust::embed::RuntimeOptions;
use escode_host_runtime::{
    FrameSubscription, Instance, Instances, LifecycleEvent, Link, RUNTIME_CLOSED_MESSAGE,
    RuntimeClosed, RuntimeState,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// 失败保护：正常路径都在通道上等到事件，不会接近这个值。
const GUARD: Duration = Duration::from_secs(60);

struct Workspace {
    root: tempfile::TempDir,
    workspace: PathBuf,
    data_dir: PathBuf,
    home: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let workspace = root.path().join("workspace");
        let data_dir = root.path().join("data");
        let home = root.path().join("home");
        for dir in [&workspace, &data_dir, &home] {
            std::fs::create_dir_all(dir).expect("fixture dir");
        }
        Self {
            root,
            workspace,
            data_dir,
            home,
        }
    }

    fn identity(&self) -> String {
        self.workspace.to_string_lossy().into_owned()
    }

    /// 模型端点指向本 workspace 的挂起桩。
    fn options(&self, model_url: &str) -> RuntimeOptions {
        let config = self.root.path().join("model.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({
                "providerId": "fixture",
                "modelId": "fixture-model",
                "reasoningLevel": "none",
                "baseUrl": model_url,
            }))
            .expect("config json"),
        )
        .expect("model config write");
        RuntimeOptions {
            home: self.home.clone(),
            env: BTreeMap::from([("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned())]),
            argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
            cwd: self.workspace.clone(),
            workspace_identity: None,
            data_dir: Some(self.data_dir.clone()),
            config: Some(config),
        }
    }

    fn generate_params(&self, prompt: &str) -> Value {
        json!({
            "workspace": {"workspacePath": self.identity()},
            "prompt": prompt,
            "querySource": "fixture",
        })
    }
}

/// 挂起式 OpenAI SSE 模型桩：每收到一个请求就在 `seen` 上报一次，然后挂住连接，
/// 直到 `release` 收到要回的文本（发送端 drop 则一直挂到连接被对端关闭）。
struct HeldModel {
    url: String,
    seen: mpsc::UnboundedReceiver<()>,
    release: Option<oneshot::Sender<String>>,
}

async fn held_model() -> HeldModel {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("model stub bind");
    let addr = listener.local_addr().expect("model stub addr");
    let (seen_tx, seen) = mpsc::unbounded_channel();
    let (release, released) = oneshot::channel::<String>();
    let released = Arc::new(tokio::sync::Mutex::new(Some(released)));
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let seen_tx = seen_tx.clone();
            let released = Arc::clone(&released);
            tokio::spawn(async move {
                if !read_request(&mut stream).await {
                    return;
                }
                let _ = seen_tx.send(());
                let Some(released) = released.lock().await.take() else {
                    return;
                };
                let Ok(text) = released.await else {
                    // 发送端 drop：一直挂到对端关闭连接。
                    let mut sink = [0u8; 64];
                    while stream.read(&mut sink).await.unwrap_or(0) > 0 {}
                    return;
                };
                let chunk = json!({
                    "choices": [{"delta": {"content": text}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                });
                let sse = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    sse.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(sse.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    HeldModel {
        url: format!("http://{addr}/v1"),
        seen,
        release: Some(release),
    }
}

/// 读完一个 HTTP 请求（头 + content-length 的体）；对端提前关闭返回 false。
async fn read_request(stream: &mut tokio::net::TcpStream) -> bool {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return false;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|part| part == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let length = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return false;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    true
}

/// 读订阅帧直到 `startup/storageState` ready。
async fn wait_ready(frames: &mut FrameSubscription) {
    loop {
        let line = tokio::time::timeout(GUARD, frames.recv())
            .await
            .expect("frame within guard")
            .expect("instance output open");
        let value: Value = serde_json::from_str(&line).expect("NDJSON frame");
        if value["method"] == "startup/storageState" && value["params"]["phase"] == "ready" {
            return;
        }
    }
}

/// 读完订阅里已缓冲的帧，直到流结束。
async fn drain_to_end(frames: &mut FrameSubscription) {
    while tokio::time::timeout(GUARD, frames.recv())
        .await
        .expect("subscription ends within guard")
        .is_some()
    {}
}

async fn next_lifecycle(events: &mut mpsc::UnboundedReceiver<LifecycleEvent>) -> LifecycleEvent {
    tokio::time::timeout(GUARD, events.recv())
        .await
        .expect("lifecycle event within guard")
        .expect("lifecycle stream open")
}

fn spawn_call(
    instance: &Arc<Instance>,
    method: &'static str,
    params: Value,
) -> tokio::task::JoinHandle<anyhow::Result<Value>> {
    let instance = Arc::clone(instance);
    tokio::spawn(async move { instance.call(method, params, GUARD).await })
}

fn assert_runtime_closed(outcome: anyhow::Result<Value>, what: &str) {
    let error = outcome.expect_err(what);
    assert_eq!(
        error.to_string(),
        RUNTIME_CLOSED_MESSAGE,
        "{what}: {error:#}"
    );
    assert!(
        error.downcast_ref::<RuntimeClosed>().is_some(),
        "{what}: typed RuntimeClosed, got {error:#}"
    );
}

/// 崩溃方式：embedded 注入 panic，child 杀子进程。
enum Crash {
    Panic,
    Kill,
}

async fn crash_isolation(link: Link, crash: Crash) {
    let a = Workspace::new();
    let b = Workspace::new();
    let mut model_a = held_model().await;
    let mut model_b = held_model().await;
    let instances = Instances::new(link).with_grace(Duration::from_secs(10));
    let mut lifecycle = instances.lifecycle();

    let instance_a = instances
        .get(&a.identity(), || a.options(&model_a.url))
        .await
        .expect("A starts");
    let instance_b = instances
        .get(&b.identity(), || b.options(&model_b.url))
        .await
        .expect("B starts");
    let mut frames_a = instance_a.subscribe().await;
    let mut frames_b = instance_b.subscribe().await;
    wait_ready(&mut frames_a).await;
    wait_ready(&mut frames_b).await;
    for expected in [a.identity(), b.identity()] {
        let event = next_lifecycle(&mut lifecycle).await;
        assert_eq!(
            (event.identity, event.generation, event.state),
            (expected, 1, RuntimeState::Available)
        );
    }

    // 两个进行中请求：模型桩收到请求即确定它们停在 runtime 内。
    let pending_a = spawn_call(
        &instance_a,
        "workspace/generateText",
        a.generate_params("a"),
    );
    let pending_b = spawn_call(
        &instance_b,
        "workspace/generateText",
        b.generate_params("b"),
    );
    tokio::time::timeout(GUARD, model_a.seen.recv())
        .await
        .expect("A model request")
        .expect("A stub open");
    tokio::time::timeout(GUARD, model_b.seen.recv())
        .await
        .expect("B model request")
        .expect("B stub open");

    match crash {
        Crash::Panic => {
            let panicked = instance_a.call("test/panic", json!({}), GUARD).await;
            assert_runtime_closed(panicked, "the panicking request itself");
        }
        Crash::Kill => kill(instance_a.process_id().expect("child pid")),
    }

    // A：在飞请求显式失败（不等 60 s 的请求超时）、订阅结束且不是 lagged。
    assert_runtime_closed(
        pending_a.await.expect("A call task"),
        "A in-flight generateText",
    );
    drain_to_end(&mut frames_a).await;
    assert!(!frames_a.lagged(), "A 的订阅因实例退出结束，不是消费过慢");
    assert!(frames_a.is_closed());
    let unavailable = next_lifecycle(&mut lifecycle).await;
    assert_eq!(
        (
            unavailable.identity.as_str(),
            unavailable.generation,
            unavailable.state
        ),
        (a.identity().as_str(), 1, RuntimeState::Unavailable)
    );

    // 崩溃回收：不等下一次 get，残余为空。
    let report = tokio::time::timeout(GUARD, instance_a.exited())
        .await
        .expect("A reaped within guard");
    assert!(!report.requested, "崩溃，不是 Host 请求的收口: {report:?}");
    assert_eq!(report.generation, 1);
    assert_eq!(report.residual, Vec::<String>::new(), "{report:?}");
    match crash {
        Crash::Panic => assert_eq!(report.cause, "engine panicked: fault-injection: test/panic"),
        Crash::Kill => {
            #[cfg(unix)]
            assert_eq!(report.cause, "signal: 9 (SIGKILL)");
        }
    }
    assert!(!instance_a.is_alive());
    assert!(instances.live(&a.identity()).is_none());

    // B：进行中请求照常完成、后续请求照常、订阅仍开着、代次不变。
    model_b
        .release
        .take()
        .expect("B release")
        .send("from B".to_owned())
        .expect("B stub waiting");
    let answer_b = pending_b
        .await
        .expect("B call task")
        .expect("B in-flight generateText succeeds");
    assert_eq!(answer_b["result"]["text"], "from B", "{answer_b}");
    let listed_b = instance_b
        .call("session/list", json!({}), GUARD)
        .await
        .expect("B later request");
    assert!(listed_b.get("result").is_some(), "{listed_b}");
    assert!(instance_b.is_alive());
    assert!(!frames_b.is_closed(), "B 的订阅不受 A 崩溃影响");

    // A：下一次 get 懒重启第 2 代，请求照常。
    let restarted = instances
        .get(&a.identity(), || a.options(&model_a.url))
        .await
        .expect("A restarts lazily");
    assert_eq!(restarted.generation, 2);
    let mut frames_a2 = restarted.subscribe().await;
    wait_ready(&mut frames_a2).await;
    let listed_a = restarted
        .call("session/list", json!({}), GUARD)
        .await
        .expect("A request after restart");
    assert!(listed_a.get("result").is_some(), "{listed_a}");
    let available = next_lifecycle(&mut lifecycle).await;
    assert_eq!(
        (
            available.identity.as_str(),
            available.generation,
            available.state
        ),
        (a.identity().as_str(), 2, RuntimeState::Available)
    );
    assert!(
        lifecycle.try_recv().is_err(),
        "B 没有任何生命周期事件，A 恰好 available(1)/unavailable(1)/available(2)"
    );
    assert_eq!(
        instances.generations(),
        BTreeMap::from([(a.identity(), 2), (b.identity(), 1)])
    );

    let residual = instances.shutdown().await;
    assert!(residual.is_empty(), "收口无残余: {residual:?}");
    drop(model_a.release.take());
}

#[tokio::test(flavor = "multi_thread")]
async fn embedded_panic_in_one_workspace_leaves_the_other_untouched() {
    crash_isolation(Link::Embedded, Crash::Panic).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn killed_child_in_one_workspace_leaves_the_other_untouched() {
    let Some(binary) = cli_binary() else {
        // 显式 skip（M2 报告统计 skip 次数）：CI 的 test 作业先构建 CLI workspace，这里在场。
        eprintln!("skip: escode-cli-rust 二进制不在场（先构建 CLI workspace）");
        return;
    };
    crash_isolation(Link::Child { binary }, Crash::Kill).await;
}

fn cli_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "escode-cli-rust.exe"
    } else {
        "escode-cli-rust"
    };
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../escode-cli-rust/target/debug")
        .join(name);
    candidate.is_file().then_some(candidate)
}

/// 强杀子进程：Unix SIGKILL；Windows `taskkill /F`（TerminateProcess）。
fn kill(pid: u32) {
    let status = if cfg!(windows) {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status()
    } else {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
    }
    .expect("kill command");
    assert!(status.success(), "kill {pid}: {status}");
}
