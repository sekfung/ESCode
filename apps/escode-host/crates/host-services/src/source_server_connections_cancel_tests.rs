//! 首连取消（S5-a cancel，spec §12.2）：caller 作用域的取消不得污染后续重连。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::json;

use crate::source_server::SourceServerRegistry;
use crate::source_server_connections::ConnectionsOptions;

use super::*;

const PROFILE: &str = "44444444-4444-4444-8444-444444444444";

fn registry() -> Arc<SourceServerRegistry> {
    Arc::new(
        SourceServerRegistry::from_settings(&json!({
            "sourceServers": [{
                "sourceServerId": "33333333-3333-4333-8333-333333333333",
                "displayName": "build-box",
            }],
            "connectionProfiles": [{
                "profileId": PROFILE,
                "sourceServerId": "33333333-3333-4333-8333-333333333333",
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "build.example", "username": "dev"},
                "createdAt": 1_700_000_000_000_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("registry"),
    )
}

/// 首次 transport 解析永久挂起（模拟 SSH 握手被卡住），之后显式失败。
fn stalling_options(attempts: Arc<AtomicU32>) -> ConnectionsOptions {
    ConnectionsOptions::new("3.15.2", move |_profile| {
        let attempts = Arc::clone(&attempts);
        Box::pin(async move {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                std::future::pending::<()>().await;
            }
            anyhow::bail!("transport unavailable")
        })
    })
}

async fn wait_bootstrapping(registry: &SourceServerRegistry) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if registry
                .connection_state(PROFILE)
                .is_some_and(|state| state.state == "bootstrapping")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("connect reaches bootstrapping");
}

/// 取消在途连接：future 立即收口，状态改为可重试 offline，后续 connect 不被卡死。
#[tokio::test]
async fn cancel_aborts_pending_connect_and_allows_retry() {
    let registry = registry();
    let attempts = Arc::new(AtomicU32::new(0));
    let connections = Arc::new(SourceServerConnections::new(
        Arc::clone(&registry),
        stalling_options(Arc::clone(&attempts)),
    ));

    let cancel = connections.register_cancellable("caller-1");
    let task = {
        let connections = Arc::clone(&connections);
        tokio::spawn(async move { connections.connect_cancellable(PROFILE, cancel).await })
    };
    wait_bootstrapping(&registry).await;

    assert!(connections.cancel_cancellable("caller-1"));
    let error = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("cancel aborts the pending connect")
        .expect("join")
        .expect_err("cancelled connect is an error");
    assert!(error.downcast_ref::<ConnectionCanceled>().is_some());

    let state = registry.connection_state(PROFILE).expect("state");
    assert_eq!(state.state, "offline");
    assert_eq!(
        state.error_code.as_deref(),
        Some("remote-connection-canceled")
    );
    assert_eq!(state.retryable, Some(true));

    connections.finish_cancellable("caller-1");
    assert!(!connections.cancel_cancellable("caller-1"));

    // 重试不被取消的 starting 单飞卡住：进入 transport 并显式失败。
    let retry = connections
        .connect(PROFILE)
        .await
        .expect_err("transport unavailable");
    assert!(format!("{retry:#}").contains("transport unavailable"));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

/// 未知 caller key 不产生任何副作用。
#[tokio::test]
async fn cancel_unknown_caller_is_a_no_op() {
    let registry = registry();
    let connections = SourceServerConnections::new(
        registry.clone(),
        stalling_options(Arc::new(AtomicU32::new(0))),
    );
    let generation = registry.generation();
    assert!(!connections.cancel_cancellable("missing"));
    // 取消未知 caller 不发布状态（generation 不前进）。
    assert_eq!(registry.generation(), generation);
}

/// 同一个 Profile 的另一个 caller 不受取消影响（共享 single-flight 不误伤）。
#[tokio::test]
async fn cancel_only_affects_its_own_caller() {
    let registry = registry();
    let attempts = Arc::new(AtomicU32::new(0));
    let connections = Arc::new(SourceServerConnections::new(
        Arc::clone(&registry),
        stalling_options(Arc::clone(&attempts)),
    ));

    let first = connections.register_cancellable("caller-1");
    let second = connections.register_cancellable("caller-2");
    let task = {
        let connections = Arc::clone(&connections);
        tokio::spawn(async move { connections.connect_cancellable(PROFILE, first).await })
    };
    wait_bootstrapping(&registry).await;

    assert!(connections.cancel_cancellable("caller-1"));
    let _ = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("first caller aborts");

    // caller-2 的信号仍是 false；它完成后重连会走新的 transport 尝试。
    assert!(!*second.borrow());
    connections.finish_cancellable("caller-1");
    connections.finish_cancellable("caller-2");
    assert!(!connections.cancel_cancellable("caller-2"));
}

/// 取消必须真正终止 transport 子进程（kill_on_drop）；否则 SSH socket 会留在代理侧不关闭。
#[tokio::test]
async fn cancel_kills_the_pending_transport_process() {
    use crate::source_server_exec::ResolvedTransport;
    use crate::source_server_transport::TransportCommand;

    let marker = std::env::temp_dir().join(format!("escode-cancel-pid-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let script = format!(
        "echo $$ > '{}'; exec sleep 60",
        marker.to_string_lossy().replace('\'', r"'\''")
    );
    let command = TransportCommand {
        program: std::path::PathBuf::from("sh"),
        args: vec![
            std::ffi::OsString::from("-c"),
            std::ffi::OsString::from(script),
        ],
        env: Default::default(),
    };

    let registry = registry();
    let attempts = Arc::new(AtomicU32::new(0));
    let resolver_attempts = Arc::clone(&attempts);
    let connections = Arc::new(SourceServerConnections::new(
        Arc::clone(&registry),
        ConnectionsOptions::new("3.15.2", move |_profile| {
            let command = command.clone();
            let attempts = Arc::clone(&resolver_attempts);
            Box::pin(async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Ok(ResolvedTransport::command_only(command))
            })
        }),
    ));

    let cancel = connections.register_cancellable("caller-1");
    let task = {
        let connections = Arc::clone(&connections);
        tokio::spawn(async move { connections.connect_cancellable(PROFILE, cancel).await })
    };

    // 等 transport 真正启动并写出自己的 PID（握手停在 read_handshake）。
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("transport process started");

    assert!(connections.cancel_cancellable("caller-1"));
    let _ = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("cancel aborts the pending connect");

    let alive = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("transport process is reaped after cancel");
    assert!(!alive);
    connections.finish_cancellable("caller-1");
    let _ = std::fs::remove_file(&marker);
}

/// 取消可能发生在安装/预检的远程命令里：丢弃 `run_command` future 也必须终止 transport。
#[tokio::test]
async fn dropping_run_command_kills_the_transport_process() {
    use crate::source_server_exec::run_command;
    use crate::source_server_transport::TransportCommand;

    let marker = std::env::temp_dir().join(format!("escode-cancel-exec-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let script = format!(
        "echo $$ > '{}'; exec sleep 60",
        marker.to_string_lossy().replace('\'', r"'\''")
    );
    let command = TransportCommand {
        program: std::path::PathBuf::from("sh"),
        args: vec![
            std::ffi::OsString::from("-c"),
            std::ffi::OsString::from(script),
        ],
        env: Default::default(),
    };

    let mut future = Box::pin(run_command(&command, None, Duration::from_secs(60)));
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                return pid;
            }
            // 轮询 future 推动 spawn，同时等 PID 文件。
            let _ = tokio::time::timeout(Duration::from_millis(20), &mut future).await;
        }
    })
    .await
    .expect("transport process started");
    drop(future);

    let alive = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("transport process is reaped after drop");
    assert!(!alive);
    let _ = std::fs::remove_file(&marker);
}
