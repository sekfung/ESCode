//! 帧泵的非应答交互（docs/specs/rust-u2-agent-v4.md §3.3 / §4-2）：反向请求应答、
//! 实例退出时在飞请求立即失败、生命周期事件、订阅者溢出可观测。
//!
//! 用 child 链路接一个最小 shell「runtime」脚本，让每种时序都是确定的（Unix 专用）。
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use escode_cli_rust::embed::RuntimeOptions;
use escode_host_runtime::{HostError, HostResponder, Instances, Link, RuntimeState};
use serde_json::{Value, json};

struct Script {
    _root: tempfile::TempDir,
    binary: PathBuf,
    workspace: PathBuf,
}

/// 写一个忽略参数的 shell 脚本作为 runtime 子进程。
fn script(body: &str) -> Script {
    let root = tempfile::tempdir().expect("temp dir");
    let binary = root.path().join("fake-runtime");
    // 修复：本进程以写方式打开脚本时，并发用例的 fork 会继承该写句柄（exec 前才关闭），随后执行
    // 脚本得 ETXTBSY “Text file busy”（ubuntu test 作业间歇失败）。改由独立子进程写入并 chmod，
    // 本进程从不持有写句柄，子进程退出后文件不再有写者。
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"")
        .arg("sh")
        .arg(format!("#!/bin/sh\n{body}\n"))
        .arg(&binary)
        .status()
        .expect("write script");
    assert!(status.success(), "write script: {status}");
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace");
    Script {
        _root: root,
        binary,
        workspace,
    }
}

impl Script {
    fn instances(&self) -> Instances {
        Instances::new(Link::Child {
            binary: self.binary.clone(),
        })
        .with_grace(Duration::from_secs(2))
    }

    fn options(&self) -> RuntimeOptions {
        RuntimeOptions {
            home: self.workspace.clone(),
            env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            argv: vec!["fake".to_owned()],
            cwd: self.workspace.clone(),
            workspace_identity: None,
            data_dir: None,
            config: None,
        }
    }
}

/// 超时是可区分的失败类别（stale 实例恢复据此决定回收+重试）。runtime 读请求但从不应答，
/// 超时是确定的（不依赖“零期限一定先于应答”的调度假设）。
#[tokio::test]
async fn an_unanswered_request_fails_with_a_classifiable_timeout() {
    let fake = script("while read line; do :; done");
    let instances = fake.instances();
    let instance = instances.get("ws", || fake.options()).await.expect("spawn");
    let error = instance
        .call_with("plugins/list", json!({}), None, Duration::from_millis(300))
        .await
        .expect_err("runtime never answers");
    let timeout = error
        .downcast_ref::<escode_host_runtime::RequestTimeout>()
        .expect("timeout is classifiable");
    assert_eq!(timeout.method, "plugins/list");
    assert!(error.to_string().contains("did not answer `plugins/list`"));
    assert!(instance.is_alive(), "超时不等于实例退出");
    let _ = instances.shutdown().await;
}

#[tokio::test]
async fn an_in_flight_call_fails_as_soon_as_the_runtime_exits() {
    // 读到第一条请求就退出、不应答：等待方必须立即失败，而不是等满超时。
    let fake = script("read line\nexit 0");
    let instances = fake.instances();
    let instance = instances.get("ws", || fake.options()).await.expect("spawn");
    let started = Instant::now();
    let error = instance
        .call("session/list", json!({}), Duration::from_secs(20))
        .await
        .expect_err("runtime exited without answering");
    // Node 形状（docs/specs/rust-u2-host-resilience.md §2.4）：逐字相等，且是类型化的 RuntimeClosed。
    assert_eq!(
        error.to_string(),
        escode_host_runtime::RUNTIME_CLOSED_MESSAGE
    );
    assert!(
        error
            .downcast_ref::<escode_host_runtime::RuntimeClosed>()
            .is_some(),
        "unexpected error: {error:#}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "call waited {:?} instead of failing fast",
        started.elapsed()
    );
    // 关闭之后的新请求同样立即失败。
    let after = instance
        .call("session/list", json!({}), Duration::from_secs(20))
        .await
        .expect_err("closed instance");
    assert_eq!(
        after.to_string(),
        escode_host_runtime::RUNTIME_CLOSED_MESSAGE
    );
    assert!(!instance.is_alive());
}

/// 发一条反向请求，把 Host 的应答原样作为通知回显，然后退出。
const ECHO_HOST_REPLY: &str = r#"echo '{"id":"rq-1","method":"session/requestRuntimePreferences","params":{"sessionId":"s","scope":"runtime-materialization"}}'
read reply
printf '{"method":"echo","params":%s}\n' "$reply"
sleep 5"#;

async fn echoed_reply(responder: Option<HostResponder>) -> Value {
    let fake = script(ECHO_HOST_REPLY);
    let mut instances = fake.instances();
    if let Some(responder) = responder {
        instances = instances.with_responder(responder);
    }
    let instance = instances.get("ws", || fake.options()).await.expect("spawn");
    let mut frames = instance.subscribe().await;
    let line = tokio::time::timeout(Duration::from_secs(10), frames.recv())
        .await
        .expect("echo within timeout")
        .expect("echo frame");
    let _ = instances.shutdown().await;
    let echoed: Value = serde_json::from_str(&line).expect("json echo");
    assert_eq!(echoed["method"], "echo", "unexpected frame {line}");
    echoed["params"].clone()
}

#[tokio::test]
async fn reverse_requests_are_answered_by_the_injected_responder() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let responder: HostResponder = Arc::new(move |request| {
        recorder
            .lock()
            .expect("record")
            .push((request.identity.clone(), request.method));
        Box::pin(async move { Ok(json!({"memoryEnabled": true})) })
    });
    let reply = echoed_reply(Some(responder)).await;
    assert_eq!(
        reply,
        json!({"id": "rq-1", "result": {"memoryEnabled": true}})
    );
    assert_eq!(
        seen.lock().expect("seen").as_slice(),
        &[(
            "ws".to_owned(),
            "session/requestRuntimePreferences".to_owned()
        )]
    );
}

#[tokio::test]
async fn reverse_requests_without_a_responder_get_the_ts_unsupported_error() {
    let reply = echoed_reply(None).await;
    let expected = HostError::unsupported("session/requestRuntimePreferences");
    assert_eq!(
        reply,
        json!({"id": "rq-1", "error": {"code": expected.code, "message": expected.message}})
    );
    assert_eq!(
        expected.message,
        "Unsupported ESCode Protocol request: session/requestRuntimePreferences"
    );
}

#[tokio::test]
async fn lifecycle_publishes_available_then_unavailable_per_generation() {
    let fake = script("exit 0");
    let instances = fake.instances();
    let mut events = instances.lifecycle();
    let first = instances.get("ws", || fake.options()).await.expect("gen 1");
    let available = events.recv().await.expect("available");
    assert_eq!(available.state, RuntimeState::Available);
    assert_eq!(available.generation, 1);
    assert_eq!(available.identity, "ws");
    assert!(available.process_id.is_some(), "child link reports its pid");
    assert_eq!(
        available.runtime_identity,
        format!("ws:1:{}", available.process_id.unwrap_or_default())
    );
    let unavailable = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("unavailable within timeout")
        .expect("unavailable");
    assert_eq!(unavailable.state, RuntimeState::Unavailable);
    assert_eq!(unavailable.generation, 1);
    assert!(!first.is_alive());
    // 懒重启：下一代重新发布 available。
    let _second = instances.get("ws", || fake.options()).await.expect("gen 2");
    let next = events.recv().await.expect("available gen 2");
    assert_eq!((next.state, next.generation), (RuntimeState::Available, 2));
}

#[tokio::test]
async fn a_subscriber_that_falls_behind_is_dropped_and_marked_lagged() {
    // 订阅者不读，runtime 连发超过订阅容量的通知：该订阅被摘除并置 lagged（不静默丢帧）。
    let fake = script(
        r#"i=0
while [ $i -lt 6000 ]; do echo '{"method":"n"}'; i=$((i+1)); done
sleep 5"#,
    );
    let instances = fake.instances();
    let instance = instances.get("ws", || fake.options()).await.expect("spawn");
    let mut slow = instance.subscribe().await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !slow.lagged() {
        assert!(
            Instant::now() < deadline,
            "subscriber was never marked lagged"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut drained = 0;
    while slow.recv().await.is_some() {
        drained += 1;
    }
    assert!(
        drained > 0 && drained <= 4096,
        "drained {drained} buffered frames"
    );
    let _ = instances.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn available_is_announced_only_after_the_instance_is_registered() {
    // 评审 R1：监听方收到 available 时必须能按 existing-only 取到实例（`live` 非空），
    // 否则任务索引 syncer 的被动订阅会扑空进入 dormant。多次启动以放大调度交错。
    let fake = script("read line\nexit 0");
    let instances = Arc::new(fake.instances());
    let mut events = instances.lifecycle();
    let probe = Arc::clone(&instances);
    let checker = tokio::spawn(async move {
        let mut checked = 0;
        while let Some(event) = events.recv().await {
            if event.state == RuntimeState::Available {
                assert!(
                    probe.live(&event.identity).is_some(),
                    "available published before registration (generation {})",
                    event.generation
                );
                checked += 1;
                if checked == 40 {
                    return checked;
                }
            }
        }
        checked
    });
    for round in 0..40 {
        let identity = format!("ws-{round}");
        let instance = instances
            .get(&identity, || fake.options())
            .await
            .expect("spawn");
        drop(instance);
    }
    let checked = tokio::time::timeout(Duration::from_secs(30), checker)
        .await
        .expect("all announcements observed")
        .expect("checker");
    assert_eq!(checked, 40);
    let _ = instances.shutdown().await;
}
