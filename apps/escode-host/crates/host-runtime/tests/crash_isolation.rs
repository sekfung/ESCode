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
mod crash_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crash_support::{GUARD, Workspace, held_model};
use escode_host_runtime::{
    FrameSubscription, Instance, Instances, LifecycleEvent, Link, RUNTIME_CLOSED_MESSAGE,
    RuntimeClosed, RuntimeState,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

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

/// 等模型桩收到请求；在飞请求或实例先失败/退出时立刻带着真实原因失败。
///
/// 修复：此前这里只等 `seen`，Windows child 链路在请求已失败或实例已退出时仍表现为
/// 60 s 超时（`A model request: Elapsed(())`），把真正的启动/请求错误吞掉。
async fn await_model_request(
    seen: &mut mpsc::UnboundedReceiver<()>,
    pending: &mut tokio::task::JoinHandle<anyhow::Result<Value>>,
    instance: &Arc<Instance>,
    what: &str,
) {
    tokio::select! {
        seen = tokio::time::timeout(GUARD, seen.recv()) => {
            seen
                .unwrap_or_else(|_| panic!("{what} model request within {GUARD:?}"))
                .unwrap_or_else(|| panic!("{what} model stub closed"));
        }
        outcome = &mut *pending => {
            panic!("{what} request finished before reaching the model: {outcome:?}");
        }
        report = instance.exited() => {
            panic!("{what} instance exited before reaching the model: {report:?}");
        }
    }
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
    let mut pending_a = spawn_call(
        &instance_a,
        "workspace/generateText",
        a.generate_params("a"),
    );
    let mut pending_b = spawn_call(
        &instance_b,
        "workspace/generateText",
        b.generate_params("b"),
    );
    await_model_request(&mut model_a.seen, &mut pending_a, &instance_a, "A").await;
    await_model_request(&mut model_b.seen, &mut pending_b, &instance_b, "B").await;

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
    crash_isolation(
        Link::Child {
            binary: cli_binary(),
        },
        Crash::Kill,
    )
    .await;
}

fn cli_binary() -> PathBuf {
    let name = if cfg!(windows) {
        "escode-cli-rust.exe"
    } else {
        "escode-cli-rust"
    };
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../escode-cli-rust/target/debug")
        .join(name);
    // 修复（H1.8）：缺失时显式失败，不再打印 skip 后被测试框架记为 ok（“绿但未测”）。
    assert!(
        candidate.is_file(),
        "escode-cli-rust 二进制不在场：{}；先运行 \
         `cargo build --locked --manifest-path apps/escode-cli-rust/Cargo.toml --bin escode-cli-rust`",
        candidate.display()
    );
    candidate
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
