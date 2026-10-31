//! Rust `RemoteChannelClient` ↔ 真实 TS `ChannelServer` 的跨语言对拍：
//! 帧头、serialize/deserialize、Initialize 能力广播、Promise 响应、事件与错误映射。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_core::rpc_wire::FrameDecoder;
use escode_host_services::source_server_client::RemoteChannelClient;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn start_fixture() -> Child {
    let root = workspace_root();
    Command::new("node")
        .arg("--import")
        .arg("tsx")
        .arg(root.join("apps/escode-host/tests/fixtures/rpc_server_fixture.mjs"))
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("node fixture starts")
}

async fn connect() -> (std::sync::Arc<RemoteChannelClient>, Child) {
    let mut child = start_fixture();
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");
    let (outbound, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(bytes) = frames.recv().await {
            if stdin.write_all(&bytes).await.is_err() {
                return;
            }
            let _ = stdin.flush().await;
        }
    });
    let client = std::sync::Arc::new(RemoteChannelClient::new(outbound));
    let reader = std::sync::Arc::clone(&client);
    tokio::spawn(async move {
        let mut decoder = FrameDecoder::new();
        let mut chunk = [0u8; 8192];
        loop {
            let Ok(read) = stdout.read(&mut chunk).await else {
                reader.dispose();
                return;
            };
            if read == 0 {
                reader.dispose();
                return;
            }
            decoder.push(&chunk[..read]);
            loop {
                match decoder.next_frame() {
                    Ok(Some(frame)) => {
                        if let Err(error) = reader.handle_frame(&frame) {
                            eprintln!("fixture frame rejected: {error:#}");
                            reader.dispose();
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        eprintln!("fixture frame decode failed: {error}");
                        reader.dispose();
                        return;
                    }
                }
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(30), client.wait_initialized())
        .await
        .expect("initialize within timeout")
        .expect("initialize");
    (client, child)
}

#[tokio::test(flavor = "multi_thread")]
async fn talks_to_the_real_typescript_channel_server() {
    let (client, mut child) = connect().await;
    assert!(
        client.has_channel("test"),
        "Initialize 应广播已注册 channel"
    );

    let snapshot = tokio::time::timeout(
        Duration::from_secs(20),
        client.call(
            "test",
            "getSnapshot",
            Some(WireValue::Object(json!({"q": 1, "nested": [1, 2]}))),
        ),
    )
    .await
    .expect("getSnapshot within timeout")
    .expect("getSnapshot");
    assert_eq!(
        snapshot.to_json(),
        Some(json!({"ok": true, "echo": {"q": 1, "nested": [1, 2]}})),
        "参数经 TS serialize/deserialize 往返不变"
    );

    let big = tokio::time::timeout(Duration::from_secs(20), client.call("test", "big", None))
        .await
        .expect("big within timeout")
        .expect("big payload");
    match big {
        WireValue::String(text) => assert_eq!(text.len(), 100_000),
        other => panic!("big payload must be a string: {other:?}"),
    }

    let (id, mut events) = client.listen("test", "tick", None).await.expect("listen");
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event within timeout")
            .expect("event payload");
    }
    client.dispose_listener(id);

    let error = tokio::time::timeout(Duration::from_secs(20), client.call("test", "fail", None))
        .await
        .expect("fail within timeout")
        .expect_err("must fail");
    let error = error
        .downcast::<escode_host_services::source_server_client::RemoteCallError>()
        .expect("typed remote error");
    assert_eq!(error.message, "denied");
    assert_eq!(error.name, "SourceServerError");
    assert_eq!(error.data.as_ref().unwrap()["code"], json!("E_AUTH"));

    client.dispose();
    let _ = child.kill().await;
}

/// 完整 stdio 栈：TransportCommand → 子进程 → hello/ack → 帧面 → client。
#[tokio::test(flavor = "multi_thread")]
async fn connects_over_stdio_with_handshake() {
    let command = fixture_command(true);
    let connection = tokio::time::timeout(
        Duration::from_secs(30),
        escode_host_services::source_server_connection::connect(
            &command,
            &escode_host_services::source_server_connection::ConnectOptions::new(
                "desktop-test",
                "3.15.2",
            ),
        ),
    )
    .await
    .expect("connect within timeout")
    .expect("connect");
    assert_eq!(connection.info.version, "3.15.2");
    assert!(
        connection.info.pid.is_some_and(|pid| pid > 0),
        "stdio hello 携带远端 pid"
    );
    connection
        .client
        .wait_initialized()
        .await
        .expect("initialize after handshake");
    let value = connection
        .client
        .call("test", "getSnapshot", None)
        .await
        .expect("call over handshake connection");
    assert_eq!(value.to_json(), Some(json!({"ok": true, "echo": null})));
    tokio::time::timeout(Duration::from_secs(10), connection.shutdown())
        .await
        .expect("shutdown within timeout")
        .expect("shutdown");
}

/// 没有 hello 的远端显式超时，不静默重试或挂起。
#[tokio::test(flavor = "multi_thread")]
async fn missing_hello_fails_with_explicit_timeout() {
    let command = fixture_command(false);
    let options = escode_host_services::source_server_connection::ConnectOptions {
        handshake_timeout: Duration::from_millis(300),
        ..escode_host_services::source_server_connection::ConnectOptions::new(
            "desktop-test",
            "3.15.2",
        )
    };
    let error =
        match escode_host_services::source_server_connection::connect(&command, &options).await {
            Ok(_) => panic!("connect must fail without hello"),
            Err(error) => error,
        };
    assert!(
        format!("{error:#}").contains("handshake timeout"),
        "explicit handshake error: {error:#}"
    );
}

fn fixture_command(hello: bool) -> escode_host_services::source_server_transport::TransportCommand {
    fixture_command_with_env(hello, &[])
}

fn fixture_command_with_env(
    hello: bool,
    extra_env: &[(&str, &str)],
) -> escode_host_services::source_server_transport::TransportCommand {
    let root = workspace_root();
    let mut env = std::collections::BTreeMap::new();
    if hello {
        env.insert("ESCODE_FIXTURE_HELLO".to_owned(), "1".to_owned());
    }
    for (key, value) in extra_env {
        env.insert((*key).to_owned(), (*value).to_owned());
    }
    escode_host_services::source_server_transport::TransportCommand {
        program: PathBuf::from("node"),
        args: vec![
            "--import".into(),
            "tsx".into(),
            root.join("apps/escode-host/tests/fixtures/rpc_server_fixture.mjs")
                .into_os_string(),
        ],
        env,
    }
}

const MANAGED_SOURCE: &str = "33333333-3333-4333-8333-333333333333";
const MANAGED_PROFILE: &str = "44444444-4444-4444-8444-444444444444";

/// 连接管理器全链路：profile → transport → online → 调用 → 断链 → offline → 收口 absent。
async fn managed_connections() -> (
    std::sync::Arc<escode_host_services::source_server::SourceServerRegistry>,
    escode_host_services::source_server_connections::SourceServerConnections,
) {
    managed_connections_with(&[]).await
}

async fn managed_connections_with(
    extra_env: &[(&str, &str)],
) -> (
    std::sync::Arc<escode_host_services::source_server::SourceServerRegistry>,
    escode_host_services::source_server_connections::SourceServerConnections,
) {
    managed_connections_with_builder(extra_env, None, None).await
}

async fn managed_connections_with_provisioning(
    extra_env: &[(&str, &str)],
) -> (
    std::sync::Arc<escode_host_services::source_server::SourceServerRegistry>,
    escode_host_services::source_server_connections::SourceServerConnections,
) {
    let provisioning = std::sync::Arc::new(
        escode_host_services::source_server_provider_provisioning::SourceServerProviderProvisioning::new(
            std::sync::Arc::new(StaticProvisioningSource),
            Duration::from_secs(5),
        ),
    );
    managed_connections_with_builder(extra_env, Some(provisioning), None).await
}

async fn managed_connections_with_builder(
    extra_env: &[(&str, &str)],
    provisioning: Option<
        std::sync::Arc<
            escode_host_services::source_server_provider_provisioning::SourceServerProviderProvisioning,
        >,
    >,
    preferences_home: Option<PathBuf>,
) -> (
    std::sync::Arc<escode_host_services::source_server::SourceServerRegistry>,
    escode_host_services::source_server_connections::SourceServerConnections,
) {
    use escode_host_services::source_server::SourceServerRegistry;
    use escode_host_services::source_server_connections::{
        ConnectionsOptions, SourceServerConnections,
    };
    let registry = std::sync::Arc::new(
        SourceServerRegistry::from_settings(&json!({
            "sourceServers": [{
                "sourceServerId": MANAGED_SOURCE,
                "displayName": "fixture",
            }],
            "connectionProfiles": [{
                "profileId": MANAGED_PROFILE,
                "sourceServerId": MANAGED_SOURCE,
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "fixture", "username": "dev"},
                "createdAt": 1_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("registry"),
    );
    let env: Vec<(String, String)> = extra_env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    let connections = SourceServerConnections::new(
        registry.clone(),
        ConnectionsOptions::new("3.15.2", move |_| {
            let env = env.clone();
            Box::pin(async move {
                Ok(
                    escode_host_services::source_server_exec::ResolvedTransport::command_only(
                        fixture_command_with_env(
                            true,
                            &env.iter()
                                .map(|(k, v)| (k.as_str(), v.as_str()))
                                .collect::<Vec<_>>(),
                        ),
                    ),
                )
            })
        })
        .with_provisioning(provisioning)
        .with_preferences_home(preferences_home),
    );
    (registry, connections)
}

struct StaticProvisioningSource;

#[async_trait::async_trait]
impl escode_host_services::source_server_provider_provisioning::ProviderProvisioningSource
    for StaticProvisioningSource
{
    async fn read(&self, sync_id: &str) -> anyhow::Result<serde_json::Value> {
        Ok(json!({
            "schemaVersion": 2,
            "syncId": sync_id,
            "personalConfig": {
                "providerConfigRules": {"providerRules": [{
                    "providerId": "p1",
                    "config": {"group": "standard-personal"}
                }]},
                "modelConfigRules": {"providerModelRules": [], "manualProviderModelRules": []},
                "providerOrder": ["p1"]
            },
            "accountSettings": {
                "providerFamilyDomain": "zai",
                "providerFamilyConnectionSelections": {}
            },
            "credentials": [{
                "scope": "oauth-session",
                "key": "oauth:active_provider",
                "value": "zai"
            }]
        }))
    }
}

fn managed_state(
    registry: &escode_host_services::source_server::SourceServerRegistry,
) -> serde_json::Value {
    registry.snapshot().expect("snapshot")["states"][0].clone()
}

async fn next_state_event(
    events: &mut tokio::sync::broadcast::Receiver<
        escode_host_services::source_server::SourceServerEvent,
    >,
) -> serde_json::Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("state event")
            .expect("event")
        {
            escode_host_services::source_server::SourceServerEvent::State(value) => return value,
            _ => continue,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_manager_reaches_online_and_disconnects() {
    let (registry, connections) = managed_connections().await;
    let mut events = registry.subscribe();
    connections.connect(MANAGED_PROFILE).await.expect("connect");

    let bootstrapping = next_state_event(&mut events).await;
    assert_eq!(bootstrapping["state"]["state"], json!("bootstrapping"));
    let online = next_state_event(&mut events).await;
    assert_eq!(online["state"]["state"], json!("online"));
    assert_eq!(online["state"]["currentVersion"], json!("3.15.2"));
    assert_eq!(managed_state(&registry)["state"], json!("online"));
    assert_eq!(connections.connected_count(), 1);
    assert_eq!(
        connections
            .hello(MANAGED_PROFILE)
            .map(|hello| hello.version),
        Some("3.15.2".to_owned())
    );

    let client = connections.client(MANAGED_PROFILE).expect("client");
    let value = tokio::time::timeout(
        Duration::from_secs(20),
        client.call("test", "getSnapshot", None),
    )
    .await
    .expect("call within timeout")
    .expect("call");
    assert_eq!(value.to_json(), Some(json!({"ok": true, "echo": null})));

    connections.disconnect_source(MANAGED_SOURCE).await;
    assert!(connections.client(MANAGED_PROFILE).is_none());
    let state = managed_state(&registry);
    assert_eq!(state["state"], json!("absent"));
    assert!(state.get("error").is_none());
}

/// S4-g：首连在发布 online 前下发 v2 Envelope；失败走正式连接失败通道。
#[tokio::test(flavor = "multi_thread")]
async fn first_connect_applies_provisioning_before_online() {
    let dir = tempfile::tempdir().expect("dir");
    let capture = dir.path().join("provisioning.jsonl");
    let capture_text = capture.to_str().expect("capture path").to_owned();
    let (registry, connections) = managed_connections_with_provisioning(&[(
        "ESCODE_FIXTURE_PROVISIONING_CAPTURE",
        capture_text.as_str(),
    )])
    .await;
    let mut events = registry.subscribe();
    connections.connect(MANAGED_PROFILE).await.expect("connect");

    let bootstrapping = next_state_event(&mut events).await;
    assert_eq!(bootstrapping["state"]["state"], json!("bootstrapping"));
    let online = next_state_event(&mut events).await;
    assert_eq!(online["state"]["state"], json!("online"));
    assert_eq!(managed_state(&registry)["state"], json!("online"));
    assert_eq!(connections.connected_count(), 1);

    let captured = tokio::fs::read_to_string(&capture)
        .await
        .expect("capture file");
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines.len(), 1, "首连只同步一次: {captured}");
    let envelope: serde_json::Value = serde_json::from_str(lines[0]).expect("envelope");
    assert_eq!(envelope["schemaVersion"], json!(2));
    assert!(
        envelope["syncId"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        envelope["personalConfig"]["providerConfigRules"]["providerRules"][0]["providerId"],
        json!("p1")
    );
    assert_eq!(
        envelope["accountSettings"]["providerFamilyDomain"],
        json!("zai")
    );
    assert_eq!(
        envelope["credentials"][0]["key"],
        json!("oauth:active_provider")
    );

    connections.disconnect_source(MANAGED_SOURCE).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_connect_provisioning_failure_does_not_publish_online() {
    let capture_dir = tempfile::tempdir().expect("dir");
    let capture = capture_dir.path().join("provisioning.jsonl");
    let capture_text = capture.to_str().expect("capture path").to_owned();
    let (registry, connections) = managed_connections_with_provisioning(&[
        ("ESCODE_FIXTURE_PROVISIONING_CAPTURE", capture_text.as_str()),
        ("ESCODE_FIXTURE_PROVISIONING_STATUS", "failed"),
        (
            "ESCODE_FIXTURE_PROVISIONING_ERROR_CODE",
            "target-write-failed",
        ),
    ])
    .await;
    let mut events = registry.subscribe();
    let error = connections
        .connect(MANAGED_PROFILE)
        .await
        .expect_err("first connect must fail");
    assert!(format!("{error:#}").contains("provider provisioning failed"));

    let bootstrapping = next_state_event(&mut events).await;
    assert_eq!(bootstrapping["state"]["state"], json!("bootstrapping"));
    let failed = next_state_event(&mut events).await;
    assert_eq!(failed["state"]["state"], json!("failed"));
    assert_eq!(
        failed["state"]["errorCode"],
        json!("provider-provisioning-failed")
    );
    assert_eq!(managed_state(&registry)["state"], json!("failed"));
    assert_eq!(connections.connected_count(), 0);
    assert!(connections.client(MANAGED_PROFILE).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_provisioning_target_fails_first_connect() {
    let (registry, connections) =
        managed_connections_with_provisioning(&[("ESCODE_FIXTURE_PROVISIONING_CHANNEL", "0")])
            .await;
    let error = connections
        .connect(MANAGED_PROFILE)
        .await
        .expect_err("first connect must fail");
    assert!(format!("{error:#}").contains("provider provisioning unsupported"));
    let state = managed_state(&registry);
    assert_eq!(state["state"], json!("failed"));
    assert_eq!(
        state["errorCode"],
        json!("provider-provisioning-unsupported")
    );
    assert!(connections.client(MANAGED_PROFILE).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn post_online_provisioning_failure_keeps_connection_online() {
    use escode_host_services::source_server_provider_provisioning::ProvisioningTrigger;

    let dir = tempfile::tempdir().expect("dir");
    let capture = dir.path().join("provisioning.jsonl");
    let capture_text = capture.to_str().expect("capture path").to_owned();
    let (registry, connections) = managed_connections_with_provisioning(&[
        ("ESCODE_FIXTURE_PROVISIONING_CAPTURE", capture_text.as_str()),
        (
            "ESCODE_FIXTURE_PROVISIONING_STATUS_SEQUENCE",
            "applied,failed",
        ),
        (
            "ESCODE_FIXTURE_PROVISIONING_ERROR_CODE",
            "target-write-failed",
        ),
    ])
    .await;
    connections.connect(MANAGED_PROFILE).await.expect("connect");
    assert_eq!(managed_state(&registry)["state"], json!("online"));
    assert_eq!(connections.connected_count(), 1);

    connections.notify_provider_provisioning(ProvisioningTrigger::PersonalConfig);
    let ok = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let captured = tokio::fs::read_to_string(&capture)
                .await
                .unwrap_or_default();
            if captured.lines().count() >= 2 {
                break true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("trailing provisioning");
    assert!(ok);
    // 已 online 后的失败不拆连接，也不改判状态。
    assert_eq!(managed_state(&registry)["state"], json!("online"));
    assert_eq!(connections.connected_count(), 1);
    assert!(connections.client(MANAGED_PROFILE).is_some());
    connections.disconnect_source(MANAGED_SOURCE).await;
}

/// S4-g：远端 `escode-agent` 动态请求由 Rust Host 用本地 settings 应答；断线后停止应答。
#[tokio::test(flavor = "multi_thread")]
async fn remote_runtime_preferences_requests_use_local_settings() {
    let dir = tempfile::tempdir().expect("dir");
    let home = dir.path().join("home");
    let settings_dir = home.join(".escode").join("v2");
    tokio::fs::create_dir_all(&settings_dir)
        .await
        .expect("settings dir");
    tokio::fs::write(
        settings_dir.join("setting.json"),
        serde_json::to_vec(&json!({
            "memoryEnabled": true,
            "askUserQuestionAutoResolutionEnabled": false
        }))
        .expect("settings json"),
    )
    .await
    .expect("settings file");
    let capture = dir.path().join("runtime-preferences.jsonl");
    let capture_text = capture.to_str().expect("capture path").to_owned();
    let (_registry, connections) = managed_connections_with_builder(
        &[
            ("ESCODE_FIXTURE_RUNTIME_PREFERENCES", "1"),
            (
                "ESCODE_FIXTURE_RUNTIME_PREFERENCES_CAPTURE",
                capture_text.as_str(),
            ),
            ("ESCODE_FIXTURE_RUNTIME_PREFERENCES_INTERVAL_MS", "100"),
        ],
        None,
        Some(home),
    )
    .await;
    connections.connect(MANAGED_PROFILE).await.expect("connect");

    let first = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&capture).await
                && let Some(line) = text.lines().next()
            {
                break line.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("first response");
    let envelope: serde_json::Value = serde_json::from_str(&first).expect("response json");
    assert_eq!(envelope["requestId"], json!("preferences-request-1"));
    assert_eq!(envelope["resolution"]["status"], json!("resolved"));
    let preferences = &envelope["resolution"]["preferences"];
    assert_eq!(preferences["memoryEnabled"], json!(true));
    assert_eq!(
        preferences["askUserQuestionAutoResolutionEnabled"],
        json!(false)
    );
    assert_eq!(
        preferences["modelContextBudgetStrategy"],
        json!("preflight-v1")
    );
    assert!(
        preferences.get("integratedTerminalShell").is_none(),
        "runtime-materialization must not carry a user-execution shell"
    );

    connections.disconnect_source(MANAGED_SOURCE).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let baseline = tokio::fs::read_to_string(&capture)
        .await
        .unwrap_or_default()
        .lines()
        .count();
    tokio::time::sleep(Duration::from_millis(350)).await;
    let after = tokio::fs::read_to_string(&capture)
        .await
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        after, baseline,
        "closed connection must not answer further requests"
    );
    assert!(connections.client(MANAGED_PROFILE).is_none());
}

/// S4-f：迁移统计经真实 TS `source-server-workspace` 读取；报告/重试/放弃原样透传。
#[tokio::test(flavor = "multi_thread")]
async fn migration_counts_and_reports_use_remote_workspace_owner() {
    use escode_host_services::source_server::SourceServerEvent;

    let (registry, connections) =
        managed_connections_with(&[("ESCODE_FIXTURE_MIGRATION_COUNTS_DELAY_MS", "200")]).await;
    let connections = std::sync::Arc::new(connections);
    let mut events = registry.subscribe();
    connections.connect(MANAGED_PROFILE).await.expect("connect");

    let first = connections.snapshot(false).expect("snapshot");
    assert_eq!(first["migrationCounts"][0]["status"], json!("loading"));
    assert_eq!(first["failedPathCount"], json!(0));
    assert_eq!(first["abandonedPathCount"], json!(0));

    let generation = loop {
        match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("directory event")
            .expect("event")
        {
            SourceServerEvent::Directory(generation) => break generation,
            _ => continue,
        }
    };
    assert!(generation > 1);
    let second = connections.snapshot(false).expect("snapshot");
    assert_eq!(second["migrationCounts"][0]["status"], json!("available"));
    assert_eq!(second["migrationCounts"][0]["failedPathCount"], json!(2));
    assert_eq!(second["migrationCounts"][0]["abandonedPathCount"], json!(3));
    assert_eq!(second["failedPathCount"], json!(2));
    assert_eq!(second["abandonedPathCount"], json!(3));

    let report = connections
        .migration_report(MANAGED_SOURCE)
        .await
        .expect("migration report");
    assert_eq!(report["rows"][0]["migrationId"], json!("migration-1"));
    let abandoned = connections
        .abandon_workspace_migration(MANAGED_SOURCE, "/remote/workspace", "migration-1")
        .await
        .expect("abandon");
    assert_eq!(abandoned["rows"][0]["status"], json!("completed"));
    let retried = connections
        .retry_workspace_migration(MANAGED_SOURCE, "/remote/workspace", "migration-1")
        .await
        .expect("retry");
    assert_eq!(retried["rows"][0]["migrationId"], json!("migration-1"));

    connections.invalidate_migration_counts(MANAGED_SOURCE);
    let refreshed = connections.snapshot(false).expect("snapshot");
    assert_eq!(refreshed["migrationCounts"][0]["status"], json!("loading"));

    connections.disconnect_source(MANAGED_SOURCE).await;
    let offline = connections.snapshot(false).expect("snapshot");
    assert_eq!(
        offline["migrationCounts"][0]["status"],
        json!("requires-connection")
    );
    assert_eq!(offline["failedPathCount"], json!(0));
}

/// 链路断开（远端进程退出）改判 `offline`，不删除历史、不自动重连。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn link_loss_marks_offline() {
    let (registry, connections) = managed_connections().await;
    connections.connect(MANAGED_PROFILE).await.expect("connect");
    let pid = connections
        .hello(MANAGED_PROFILE)
        .expect("hello")
        .pid
        .expect("stdio hello 携带 pid")
        .to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("kill fixture");
    assert!(killed.success(), "fixture terminates");

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let state = managed_state(&registry);
        if state["state"] == json!("offline") {
            assert_eq!(state["retryable"], json!(true));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "link loss must mark offline, last state: {state}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(connections.client(MANAGED_PROFILE).is_none());
}
