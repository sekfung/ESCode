//! S4-e 自动恢复与启动自动连接测试（spec §11.5）。
//!
//! 链路断开用 fixture 的 `ESCODE_FIXTURE_EXIT_AFTER_MS` 模拟；后续尝试注入失败命令，
//! 不依赖真实远端。

#[cfg(unix)]
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::bail;
use serde_json::{Value, json};

#[cfg(unix)]
use super::test_support::*;
#[cfg(unix)]
use crate::source_server::SourceServerEvent;
use crate::source_server_connection_state::now_ms;
#[cfg(unix)]
use crate::source_server_connection_state::{RECOVERY_BUDGET_EXHAUSTED_CODE, RECOVERY_LIMIT};
use crate::source_server_connections::{
    ConnectionsOptions, SourceServerConnections, TransportFuture,
};
#[cfg(unix)]
use crate::source_server_exec::ResolvedTransport;
use crate::source_server_recovery::STARTUP_RECONNECT_WINDOW_MS;
#[cfg(unix)]
use crate::source_server_transport::TransportCommand;

#[cfg(unix)]
fn exit_after(ms: &str) -> TransportCommand {
    fixture_command_with_env(&[("ESCODE_FIXTURE_EXIT_AFTER_MS", ms)])
}

#[cfg(unix)]
fn failing_command() -> TransportCommand {
    TransportCommand {
        program: "sh".into(),
        args: vec!["-c".into(), "exit 7".into()],
        env: BTreeMap::new(),
    }
}

#[cfg(unix)]
fn manager(
    registry: Arc<crate::source_server::SourceServerRegistry>,
    runner: Arc<LocalRunner>,
    commands: Vec<TransportCommand>,
    delays: [Duration; 3],
) -> Arc<SourceServerConnections> {
    let commands = Arc::new(commands);
    let cursor = Arc::new(AtomicUsize::new(0));
    let options = ConnectionsOptions::new("3.15.2", move |_| {
        let commands = Arc::clone(&commands);
        let cursor = Arc::clone(&cursor);
        let runner = Arc::clone(&runner);
        Box::pin(async move {
            let index = cursor.fetch_add(1, Ordering::Relaxed);
            Ok(ResolvedTransport {
                command: commands[index.min(commands.len() - 1)].clone(),
                runner,
            })
        })
    })
    .with_recovery_delays(delays)
    .with_startup_timeout(Duration::from_millis(200));
    let connections = Arc::new(SourceServerConnections::new(registry, options));
    SourceServerConnections::install_self(&connections);
    connections
}

#[cfg(unix)]
async fn wait_state(
    registry: &crate::source_server::SourceServerRegistry,
    predicate: impl Fn(&crate::source_server::ConnectionStateSnapshot) -> bool,
) -> crate::source_server::ConnectionStateSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(state) = registry.connection_state(PROFILE)
                && predicate(&state)
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("state within timeout")
}

#[cfg(unix)]
fn close_and_collect(
    events: &mut tokio::sync::broadcast::Receiver<SourceServerEvent>,
) -> Vec<Value> {
    let mut states = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let SourceServerEvent::State(value) = event {
            states.push(value);
        }
    }
    states
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_drop_retries_then_returns_online() {
    let temp = tempfile::tempdir().expect("temp");
    let runner = Arc::new(LocalRunner::new(temp.path().join("home")));
    let registry = registry();
    let mut events = registry.subscribe();
    let connections = manager(
        Arc::clone(&registry),
        runner,
        vec![exit_after("400"), fixture_command()],
        [
            Duration::from_millis(20),
            Duration::from_millis(20),
            Duration::from_millis(20),
        ],
    );

    connections.connect(PROFILE).await.expect("initial connect");
    let state = wait_state(&registry, |state| {
        state.state == "online" && connections.connected_count() >= 2
    })
    .await;
    assert_eq!(state.state, "online");

    let states = close_and_collect(&mut events);
    let retrying: Vec<&Value> = states
        .iter()
        .filter(|event| event["state"]["state"] == json!("retrying"))
        .collect();
    assert_eq!(retrying.len(), 1, "首次重试成功即停止");
    assert_eq!(retrying[0]["state"]["recoveryAttempt"], json!(1));
    assert_eq!(retrying[0]["state"]["recoveryLimit"], json!(RECOVERY_LIMIT));
    assert!(
        retrying[0]["state"]
            .get("recoveryBudgetExhausted")
            .is_none(),
        "恢复中不标记预算耗尽"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exhausted_recovery_reports_offline_with_budget_code() {
    let temp = tempfile::tempdir().expect("temp");
    let runner = Arc::new(LocalRunner::new(temp.path().join("home")));
    let registry = registry();
    let mut events = registry.subscribe();
    let connections = manager(
        Arc::clone(&registry),
        runner,
        vec![exit_after("250"), failing_command()],
        [
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        ],
    );

    connections.connect(PROFILE).await.expect("initial connect");
    let state = wait_state(&registry, |state| {
        state.state == "offline"
            && state.error_code.as_deref() == Some(RECOVERY_BUDGET_EXHAUSTED_CODE)
    })
    .await;
    assert_eq!(state.recovery_attempt, Some(RECOVERY_LIMIT));
    assert_eq!(state.recovery_limit, Some(RECOVERY_LIMIT));
    assert_eq!(state.recovery_budget_exhausted, Some(true));
    assert_eq!(connections.connected_count(), 1, "失败尝试不建立连接");

    let states = close_and_collect(&mut events);
    let attempts: Vec<u32> = states
        .iter()
        .filter(|event| event["state"]["state"] == json!("retrying"))
        .filter_map(|event| event["state"]["recoveryAttempt"].as_u64())
        .map(|attempt| attempt as u32)
        .collect();
    assert_eq!(attempts, vec![1, 2, 3], "每档延迟前都发布 retrying");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_disconnect_cancels_pending_recovery() {
    let temp = tempfile::tempdir().expect("temp");
    let runner = Arc::new(LocalRunner::new(temp.path().join("home")));
    let registry = registry();
    let connections = manager(
        Arc::clone(&registry),
        runner,
        vec![exit_after("250"), failing_command()],
        [
            Duration::from_millis(300),
            Duration::from_millis(300),
            Duration::from_millis(300),
        ],
    );

    connections.connect(PROFILE).await.expect("initial connect");
    wait_state(&registry, |state| state.state == "retrying").await;
    connections.disconnect(PROFILE).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let state = registry.connection_state(PROFILE).expect("state");
    assert_eq!(state.state, "absent");
    assert_eq!(state.recovery_attempt, None, "手动收口不伪造自动重试计数");
    assert_eq!(connections.connected_count(), 1, "取消后不再尝试连接");
}

fn startup_registry(count: usize) -> Arc<crate::source_server::SourceServerRegistry> {
    let servers: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "sourceServerId": format!("source-{index}"),
                "displayName": format!("source {index}"),
                "lastOnlineAt": now_ms(),
            })
        })
        .collect();
    let profiles: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "profileId": format!("profile-{index}"),
                "sourceServerId": format!("source-{index}"),
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "h", "username": "u"},
                "createdAt": 1_u64,
            })
        })
        .collect();
    Arc::new(
        crate::source_server::SourceServerRegistry::from_settings(&json!({
            "sourceServers": servers,
            "connectionProfiles": profiles,
        }))
        .expect("registry"),
    )
}

fn counting_manager(
    registry: Arc<crate::source_server::SourceServerRegistry>,
    calls: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
) -> Arc<SourceServerConnections> {
    let resolver = move |_: &crate::source_server::ConnectionProfile| {
        calls.fetch_add(1, Ordering::Relaxed);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        Box::pin(async move {
            let current = active.fetch_add(1, Ordering::Relaxed) + 1;
            peak.fetch_max(current, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(30)).await;
            active.fetch_sub(1, Ordering::Relaxed);
            bail!("startup fixture fails")
        }) as TransportFuture
    };
    Arc::new(SourceServerConnections::new(
        registry,
        ConnectionsOptions::new("3.15.2", resolver).with_startup_timeout(Duration::from_secs(2)),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_reconnect_is_bounded_and_attempts_each_source_once() {
    let registry = startup_registry(6);
    let calls = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let connections = counting_manager(
        Arc::clone(&registry),
        Arc::clone(&calls),
        Arc::clone(&active),
        Arc::clone(&peak),
    );
    let candidates = registry.startup_candidates(now_ms(), STARTUP_RECONNECT_WINDOW_MS);
    assert_eq!(candidates.len(), 6);

    connections.startup_reconnect(candidates).await;

    assert_eq!(calls.load(Ordering::Relaxed), 6, "每 Source 只试一次");
    assert!(peak.load(Ordering::Relaxed) <= 3, "启动自动连接 ≤3 并发");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_timeout_publishes_failed_timeout_state() {
    let registry = startup_registry(1);
    let runner = Arc::new(LocalRunner::new(
        tempfile::tempdir().expect("temp").path().join("h"),
    ));
    let command = TransportCommand {
        program: "node".into(),
        args: vec!["-e".into(), "setTimeout(() => {}, 30000)".into()],
        env: BTreeMap::new(),
    };
    let connections = manager(
        Arc::clone(&registry),
        runner,
        vec![command],
        [Duration::from_millis(10); 3],
    );
    let candidates = registry.startup_candidates(now_ms(), STARTUP_RECONNECT_WINDOW_MS);
    assert_eq!(candidates.len(), 1);

    connections.startup_reconnect(candidates).await;

    let state = registry
        .connection_state("profile-0")
        .expect("timeout state");
    assert_eq!(state.state, "failed");
    assert_eq!(
        state.error_code.as_deref(),
        Some("remote-connection-timeout")
    );
}
