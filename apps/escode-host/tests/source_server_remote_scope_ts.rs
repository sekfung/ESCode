//! 跨语言验收（spec §10.11 验收 3）：真实 TS `ChannelServer` fixture 经 Host `serve()` +
//! scoped capability 完成一次方法往返与一次事件转发。
//!
//! 链路：HTTP 申请 capability（body 带 scope）→ `SourceServerConnections` 启动 fixture 并握手 →
//! WebSocket `/ws/host` 消费 capability → 远端 channel 的调用/订阅被转发到 fixture。
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use escode_host_rpc::frame::FrameReader;
use escode_host_rpc::session::{self, Channel, Channels, handler_rpc};
use escode_host_rpc::wire::{Reader, Wire, Writer};
use escode_host_services::source_server::SourceServerRegistry;
use escode_host_services::source_server_connections::{
    ConnectionsOptions, SourceServerConnections,
};
use escode_host_services::source_server_transport::TransportCommand;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
const PROFILE: &str = "44444444-4444-4444-8444-444444444444";
const IDENTITY: &str = "remote:server:33333333-3333-4333-8333-333333333333:/remote/workspace";
const REMOTE_CHANNEL: &str = "window-project-changes";
const TASK_CHANNEL: &str = "escode-task";
const LOCAL_CHANNEL: &str = "setting";

fn workspace_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root");
    // 修复：Windows 的 `canonicalize` 回 `\\?\D:\...`（verbatim 前缀），把它当脚本参数交给 `node`
    // 会被 `Module._findPath` 解析成主机 `D:`（EISDIR），windows-latest 的跨语言对拍因此全红。
    // 这里剥离非 UNC 的 verbatim 前缀，得到与 Node `realpath` 一致的 `D:\...`
    // （docs/specs/rust-windows-parity.md「测试夹具同样禁止把 verbatim 路径交给子进程」）。
    #[cfg(windows)]
    let root = match root.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => root,
    };
    root
}

fn fixture_command(extra_env: &[(String, String)]) -> TransportCommand {
    let root = workspace_root();
    let mut env = std::collections::BTreeMap::new();
    env.insert("ESCODE_FIXTURE_HELLO".to_owned(), "1".to_owned());
    env.extend(extra_env.iter().cloned());
    TransportCommand {
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

async fn start_surface(extra_env: Vec<(String, String)>) -> escode_host_rpc::RpcSurface {
    let registry = Arc::new(
        SourceServerRegistry::from_settings(&json!({
            "sourceServers": [{ "sourceServerId": SOURCE, "displayName": "fixture" }],
            "connectionProfiles": [{
                "profileId": PROFILE,
                "sourceServerId": SOURCE,
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "fixture", "username": "dev"},
                "createdAt": 1_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("registry"),
    );
    let connections = Arc::new(SourceServerConnections::new(
        registry,
        ConnectionsOptions::new("3.15.2", move |_| {
            let extra_env = extra_env.clone();
            Box::pin(async move {
                Ok(
                    escode_host_services::source_server_exec::ResolvedTransport::command_only(
                        fixture_command(&extra_env),
                    ),
                )
            })
        }),
    ));
    // 本地共享 channel 的代表：`setting` 不得出现在远端 scope；允许表里的 `window-controller` 保留。
    let local = |name: &'static str| {
        Channel::new(
            name,
            vec![(
                "get",
                handler_rpc(|_arg: Wire| {
                    Box::pin(async { Ok(Wire::Object(json!({ "local": true }))) })
                }),
            )],
        )
    };
    let channels = Channels::new(vec![local(LOCAL_CHANNEL), local("window-controller")])
        .with_remote_scope(
            escode_host_rpc::remote_scope_ensurer(connections),
            escode_host_rpc::REMOTE_SCOPE_HOST_CHANNELS.to_vec(),
        )
        .with_scoped_set(escode_host_rpc::remote_scope_channel_set());
    escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, None)
        .await
        .expect("rpc surface starts")
}

async fn http_post(
    addr: std::net::SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, serde_json::Value) {
    let mut stream = TcpStream::connect(addr).await.expect("http connect");
    let mut request = format!("POST {path} HTTP/1.1\r\nhost: {addr}\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, serde_json::from_str(&body).unwrap_or_default())
}

async fn ws_connect(addr: std::net::SocketAddr, capability: &str) -> Socket {
    ws_connect_at(addr, "/ws/host", capability).await
}

/// 按路径连接：`/ws/host` 是 desktop-continuous，`/ws` 是 web-remote-replayable（spec §10.15）。
async fn ws_connect_at(addr: std::net::SocketAddr, path: &str, capability: &str) -> Socket {
    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://{addr}{path}"))
        .header("host", addr.to_string())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header(escode_host_rpc::http::CAPABILITY_HEADER, capability)
        .body(())
        .expect("request builds");
    let stream = TcpStream::connect(addr).await.expect("ws connect");
    let (socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("ws upgrade");
    socket
}

/// 读一帧（跳过 Initialize 的字段由调用方决定）。
async fn next_frame(socket: &mut Socket, frames: &mut FrameReader) -> (i64, i64, Wire) {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("frame within timeout")
            .expect("socket open")
            .expect("frame ok");
        let bytes: Vec<u8> = match message {
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Text(text) => text.as_bytes().to_vec(),
            _ => continue,
        };
        frames.push(&bytes);
        if let Some(body) = frames.next_frame().expect("frame parses") {
            let mut reader = Reader::new(&body);
            let Wire::Array(header) = reader.value().expect("header") else {
                panic!("response header must be an array")
            };
            let kind = header[0].as_int().expect("kind");
            let id = header.get(1).and_then(Wire::as_int).unwrap_or(-1);
            let payload = reader.value().unwrap_or(Wire::Undefined);
            return (kind, id, payload);
        }
    }
}

async fn send_call(socket: &mut Socket, id: i64, channel: &str, method: &str) {
    send_call_arg(socket, id, channel, method, serde_json::Value::Null).await;
}

/// 与 `ProxyChannel` 一致：实参打包成 `[params]`。
async fn send_call_arg(
    socket: &mut Socket,
    id: i64,
    channel: &str,
    method: &str,
    params: serde_json::Value,
) {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_PROMISE),
        Wire::Int(id),
        Wire::Str(channel.to_owned()),
        Wire::Str(method.to_owned()),
    ]));
    let arg = match params {
        serde_json::Value::Null => Wire::Array(vec![]),
        params => Wire::Array(vec![Wire::Object(params)]),
    };
    writer.value(&arg);
    let frame = escode_host_rpc::frame::frame(&writer.into_bytes());
    socket
        .send(Message::Binary(frame.into()))
        .await
        .expect("call sent");
}

async fn send_listen(socket: &mut Socket, id: i64, channel: &str, event: &str) {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_EVENT_LISTEN),
        Wire::Int(id),
        Wire::Str(channel.to_owned()),
        Wire::Str(event.to_owned()),
    ]));
    writer.value(&Wire::Array(vec![]));
    let frame = escode_host_rpc::frame::frame(&writer.into_bytes());
    socket
        .send(Message::Binary(frame.into()))
        .await
        .expect("listen sent");
}

async fn send_dispose(socket: &mut Socket, id: i64) {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_EVENT_DISPOSE),
        Wire::Int(id),
    ]));
    writer.value(&Wire::Undefined);
    let frame = escode_host_rpc::frame::frame(&writer.into_bytes());
    socket
        .send(Message::Binary(frame.into()))
        .await
        .expect("dispose sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_capability_round_trips_and_forwards_events_to_the_ts_server() {
    let surface = start_surface(Vec::new()).await;
    let (status, issued) = http_post(
        surface.addr,
        "/api/rpc-host-capability",
        &[(escode_host_rpc::http::HOST_SECRET_HEADER, &surface.secret)],
        &json!({
            "scope": {
                "sourceServerId": SOURCE,
                "profileId": PROFILE,
                "workspaceIdentity": IDENTITY,
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "scope 连接成功后签发 capability: {issued}");
    let capability = issued["capability"].as_str().expect("capability string");
    let mut socket = ws_connect(surface.addr, capability).await;
    let mut frames = FrameReader::new();

    let (kind, _, payload) = next_frame(&mut socket, &mut frames).await;
    assert_eq!(kind, session::RESPONSE_INITIALIZE);
    let Wire::Object(initialize) = payload else {
        panic!("Initialize payload is an object");
    };
    let mut names: Vec<String> = initialize["channels"]
        .as_array()
        .expect("channel list")
        .iter()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            TASK_CHANNEL.to_owned(),
            // S4-f：fixture 注册并广播 `source-server-workspace`，允许表随之纳入远端 scope。
            "source-server-workspace".to_owned(),
            "window-controller".to_owned(),
            REMOTE_CHANNEL.to_owned()
        ],
        "远端 scope 只暴露允许表的本地 channel + fixture 广播过的 inventory channel"
    );

    // 方法往返：Host 转发到 fixture 的 ChannelServer，响应按 WireValue 转回。
    send_call(&mut socket, 1, REMOTE_CHANNEL, "getCapabilities").await;
    let (kind, id, payload) = next_frame(&mut socket, &mut frames).await;
    assert_eq!(
        kind,
        session::RESPONSE_PROMISE_SUCCESS,
        "调用成功: {payload:?}"
    );
    assert_eq!(id, 1);
    let Wire::Object(snapshot) = payload else {
        panic!("fixture returns an object");
    };
    assert_eq!(snapshot["ok"], json!(true));

    // 事件：fixture 的 listen 每 10ms fire 一次；Host 必须把 EventFire 转给 Renderer。
    send_listen(&mut socket, 2, REMOTE_CHANNEL, "onDidChange").await;
    let (kind, id, payload) = next_frame(&mut socket, &mut frames).await;
    assert_eq!(kind, session::RESPONSE_EVENT_FIRE, "事件转发: {payload:?}");
    assert_eq!(id, 2);
    let Wire::Object(event) = payload else {
        panic!("fixture event payload is an object");
    };
    assert_eq!(event["event"], json!("onDidChange"));

    // 销毁订阅：连接关闭后远端订阅被 dispose（fixture 进程随之随 Host 收口退出）。
    send_dispose(&mut socket, 2).await;
    drop(socket);
    surface.shutdown().await;
}

/// 跳到指定 Promise 回包，并记录期间到达的最新 controller 快照帧。
async fn read_until_promise(
    socket: &mut Socket,
    frames: &mut FrameReader,
    id: i64,
    snapshot: &mut Option<serde_json::Value>,
    delta: &mut Option<serde_json::Value>,
) -> (i64, serde_json::Value) {
    loop {
        let (kind, frame_id, payload) = next_frame(socket, frames).await;
        if kind == session::RESPONSE_EVENT_FIRE {
            if let Wire::Object(frame) = &payload
                && frame["payload"]["kind"] == serde_json::json!("snapshot")
            {
                *snapshot = Some(frame.clone());
            }
            if let Wire::Object(frame) = &payload
                && frame["payload"]["kind"] == serde_json::json!("deltas")
            {
                *delta = Some(frame.clone());
            }
            continue;
        }
        if frame_id == id
            && (kind == session::RESPONSE_PROMISE_SUCCESS
                || kind == session::RESPONSE_PROMISE_ERROR)
        {
            match payload {
                Wire::Object(value) => return (kind, value),
                other => panic!("Promise 回包必须是对象: {other:?}"),
            }
        }
    }
}

/// 等待下一帧 controller 快照（订阅 ack 可能先于早期缓冲补发到达）。
async fn next_snapshot(socket: &mut Socket, frames: &mut FrameReader) -> serde_json::Value {
    loop {
        let (kind, _, payload) = next_frame(socket, frames).await;
        if kind != session::RESPONSE_EVENT_FIRE {
            continue;
        }
        if let Wire::Object(frame) = payload
            && frame["payload"]["kind"] == json!("snapshot")
        {
            return frame;
        }
    }
}

/// 等待下一帧 controller 增量（写后事件可能晚于 Promise ack 到达）。
async fn next_delta(socket: &mut Socket, frames: &mut FrameReader) -> serde_json::Value {
    loop {
        let (kind, _, payload) = next_frame(socket, frames).await;
        if kind != session::RESPONSE_EVENT_FIRE {
            continue;
        }
        if let Wire::Object(frame) = payload
            && frame["payload"]["kind"] == json!("deltas")
        {
            return frame;
        }
    }
}

/// 验收 1–3：远端 scope 的 `window-controller` 读取/写入真实 TS `escode-task` 事实源，
/// 快照行地址带 `sourceServerId`，且其它 workspace 的写入被显式拒绝。
/// S3-c-3b（spec §10.15）：V4 由 Source Server 权威产出，Rust Host 只转发。
/// 同一 fixture 分别以 continuous/replayable 运行时，hello、订阅帧与命令参数都必须原样到达，
/// Host 不能把某个模式写死，也不能把 replayable 恢复逻辑拼进 continuous 链路。
#[tokio::test(flavor = "multi_thread")]
async fn v4_delivery_facts_are_forwarded_without_host_rewriting() {
    for (path, mode, profile, origin) in [
        ("/ws/host", "desktop-continuous", "continuous", "desktop"),
        ("/ws", "web-remote-replayable", "replayable", "mobile"),
    ] {
        let surface =
            start_surface(vec![("ESCODE_FIXTURE_V4_MODE".to_owned(), mode.to_owned())]).await;
        let (status, issued) = http_post(
            surface.addr,
            "/api/rpc-host-capability",
            &[(escode_host_rpc::http::HOST_SECRET_HEADER, &surface.secret)],
            &json!({
                "scope": {
                    "sourceServerId": SOURCE,
                    "profileId": PROFILE,
                    "workspaceIdentity": IDENTITY,
                }
            })
            .to_string(),
        )
        .await;
        assert_eq!(status, 200, "scope 连接成功后签发 capability: {issued}");
        let capability = issued["capability"].as_str().expect("capability string");
        let mut socket = ws_connect_at(surface.addr, path, capability).await;
        let mut frames = FrameReader::new();

        let (kind, _, payload) = next_frame(&mut socket, &mut frames).await;
        assert_eq!(kind, session::RESPONSE_INITIALIZE);
        let Wire::Object(initialize) = payload else {
            panic!("Initialize payload is an object");
        };
        let names: Vec<&str> = initialize["channels"]
            .as_array()
            .expect("channel list")
            .iter()
            .filter_map(|name| name.as_str())
            .collect();
        assert!(
            names.contains(&"escode-agent"),
            "fixture 广播了 escode-agent: {names:?}"
        );

        send_call(&mut socket, 1, "escode-agent", "helloConversationV4").await;
        let (kind, id, payload) = next_frame(&mut socket, &mut frames).await;
        assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
        assert_eq!(id, 1);
        let Wire::Object(hello) = payload else {
            panic!("hello payload is an object");
        };
        assert_eq!(hello["clientMode"], json!(mode), "path={path}");
        assert_eq!(hello["deliveryProfile"], json!(profile), "path={path}");
        assert_eq!(
            hello["capabilities"]["nativeDialogs"],
            json!(mode == "desktop-continuous"),
            "path={path}"
        );

        // 先挂帧监听再订阅（Renderer 同序）；fixture 在 ack 前后各发一帧。
        send_listen(&mut socket, 2, "escode-agent", "onDynamicConversationFrame").await;
        send_call_arg(
            &mut socket,
            3,
            "escode-agent",
            "subscribeConversationV4",
            json!({
                "sessionId": "fixture-session",
                "workspacePath": "/remote/workspace",
                "workspaceIdentity": IDENTITY,
            }),
        )
        .await;
        let mut snapshot = None;
        let mut delta = None;
        let mut acked = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !acked || snapshot.is_none() || delta.is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "subscribe frames did not arrive"
            );
            let (kind, id, payload) =
                tokio::time::timeout(Duration::from_secs(5), next_frame(&mut socket, &mut frames))
                    .await
                    .expect("next frame within timeout");
            if kind == session::RESPONSE_EVENT_FIRE && id == 2 {
                let Wire::Object(frame) = payload else {
                    panic!("event payload is an object");
                };
                match frame["kind"].as_str() {
                    Some("snapshot") => snapshot = Some(frame),
                    Some("deltas") => delta = Some(frame),
                    _ => {}
                }
                continue;
            }
            if id == 3 {
                assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
                let Wire::Object(ack) = payload else {
                    panic!("subscribe ack is an object");
                };
                assert_eq!(ack["ack"]["subscriptionId"], json!("fixture-sub"));
                acked = true;
            }
        }
        let snapshot = snapshot.expect("snapshot frame forwarded");
        let delta = delta.expect("delta frame forwarded");
        for frame in [&snapshot, &delta] {
            assert_eq!(frame["topic"], json!("conversation/fixture-session"));
            assert_eq!(frame["subscriptionId"], json!("fixture-sub"));
        }

        send_call_arg(
            &mut socket,
            4,
            "escode-agent",
            "sendConversationCommandV4",
            json!({
                "workspacePath": "/remote/workspace",
                "workspaceIdentity": IDENTITY,
                "envelope": {
                    "commandId": "cmd-1",
                    "clientId": "client-1",
                    "sessionId": "fixture-session",
                    "type": "sendText",
                    "payload": {
                        "firstInput": {"text": "hello"},
                    },
                },
            }),
        )
        .await;
        loop {
            let (kind, id, payload) = next_frame(&mut socket, &mut frames).await;
            if id != 4 {
                continue;
            }
            assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
            let Wire::Object(ack) = payload else {
                panic!("command ack is an object");
            };
            assert_eq!(ack["status"], json!("accepted"));
            assert_eq!(ack["commandId"], json!("cmd-1"));
            assert_eq!(ack["clientId"], json!("client-1"));
            assert_eq!(ack["inputOrigin"], json!(origin), "path={path}");
            assert_eq!(ack["text"], json!("hello"));
            break;
        }

        // 本地 window-controller 在两种 attachment 模式下都读取同一远端 escode-task 事实。
        send_call_arg(
            &mut socket,
            5,
            "window-controller",
            "listTaskList",
            json!({
                "workspacePath": "/remote/workspace",
                "workspaceIdentity": IDENTITY,
                "kind": "active",
            }),
        )
        .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "window-controller response did not arrive"
            );
            let (kind, id, payload) =
                tokio::time::timeout(Duration::from_secs(5), next_frame(&mut socket, &mut frames))
                    .await
                    .expect("next frame within timeout");
            if id != 5 {
                continue;
            }
            assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
            let Wire::Object(list) = payload else {
                panic!("task list is an object");
            };
            let items = list["items"].as_array().expect("task list items");
            assert!(
                items
                    .iter()
                    .any(|item| item["taskId"] == json!("task-remote-1")),
                "path={path}: {list}"
            );
            break;
        }

        drop(socket);
        surface.shutdown().await;
    }
}

/// 跑 Node 侧 oracle（spec §10.16）：真实 `ChannelClient` 连 fixture，再经等价的 Node Host
/// attachment 转发，输出客户端可见的 V4 语义 JSON。
fn node_v4_oracle(mode: &str) -> serde_json::Value {
    let oracle = workspace_root().join("apps/escode-host/tests/fixtures/v4_node_oracle.mjs");
    let output = std::process::Command::new("node")
        .arg("--import")
        .arg("tsx")
        .arg(oracle)
        .env("ESCODE_FIXTURE_V4_MODE", mode)
        // oracle 直连 fixture 的 RPC 面，不参与 Host 的 hello 握手。
        .env_remove("ESCODE_FIXTURE_HELLO")
        .output()
        .expect("node oracle starts");
    assert!(
        output.status.success(),
        "node oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("oracle stdout is utf8");
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("oracle prints one JSON line");
    serde_json::from_str(line).expect("oracle JSON parses")
}

/// 去掉 Source Server 的非确定字段；其余字段必须逐项一致（spec §10.16）。
fn normalize_v4_hello(mut hello: serde_json::Value) -> serde_json::Value {
    if let Some(object) = hello.as_object_mut() {
        object.remove("serverTime");
        object.remove("connectionId");
    }
    hello
}

/// fixture 用 `setImmediate` 投递 snapshot/deltas，只排序、不丢弃任何帧。
fn sort_v4_frames(mut frames: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    frames.sort_by(|left, right| {
        let left_kind = left
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");
        let right_kind = right
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");
        left_kind
            .cmp(right_kind)
            .then_with(|| left.to_string().cmp(&right.to_string()))
    });
    frames
}

/// S3-c-3d（spec §10.16）：同一 fixture 下，Node Host 远端链路与 Rust Host 远端 scope
/// 对客户端可见的 V4 帧一致。连续（`/ws/host`）与 replayable（`/ws`）都要对拍。
#[tokio::test(flavor = "multi_thread")]
async fn v4_frames_match_the_node_remote_oracle() {
    for (path, mode) in [
        ("/ws/host", "desktop-continuous"),
        ("/ws", "web-remote-replayable"),
    ] {
        let oracle = node_v4_oracle(mode);
        let surface =
            start_surface(vec![("ESCODE_FIXTURE_V4_MODE".to_owned(), mode.to_owned())]).await;
        let (status, issued) = http_post(
            surface.addr,
            "/api/rpc-host-capability",
            &[(escode_host_rpc::http::HOST_SECRET_HEADER, &surface.secret)],
            &json!({
                "scope": {
                    "sourceServerId": SOURCE,
                    "profileId": PROFILE,
                    "workspaceIdentity": IDENTITY,
                }
            })
            .to_string(),
        )
        .await;
        assert_eq!(status, 200, "scope 连接成功后签发 capability: {issued}");
        let capability = issued["capability"].as_str().expect("capability string");
        let mut socket = ws_connect_at(surface.addr, path, capability).await;
        let mut frames = FrameReader::new();

        let (kind, _, _payload) = next_frame(&mut socket, &mut frames).await;
        assert_eq!(kind, session::RESPONSE_INITIALIZE);

        send_call(&mut socket, 1, "escode-agent", "helloConversationV4").await;
        let (kind, id, payload) = next_frame(&mut socket, &mut frames).await;
        assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
        assert_eq!(id, 1);
        let Wire::Object(hello) = payload else {
            panic!("hello payload is an object");
        };

        send_listen(&mut socket, 2, "escode-agent", "onDynamicConversationFrame").await;
        send_call_arg(
            &mut socket,
            3,
            "escode-agent",
            "subscribeConversationV4",
            json!({
                "sessionId": "fixture-session",
                "workspacePath": "/remote/workspace",
                "workspaceIdentity": IDENTITY,
            }),
        )
        .await;
        let mut ack = None;
        let mut event_frames = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while ack.is_none() || event_frames.len() < 2 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "subscribe frames did not arrive"
            );
            let (kind, id, payload) =
                tokio::time::timeout(Duration::from_secs(5), next_frame(&mut socket, &mut frames))
                    .await
                    .expect("next frame within timeout");
            if kind == session::RESPONSE_EVENT_FIRE && id == 2 {
                let Wire::Object(frame) = payload else {
                    panic!("event payload is an object");
                };
                event_frames.push(frame);
                continue;
            }
            if id == 3 {
                assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
                let Wire::Object(value) = payload else {
                    panic!("subscribe ack is an object");
                };
                ack = Some(value);
            }
        }

        send_call_arg(
            &mut socket,
            4,
            "escode-agent",
            "sendConversationCommandV4",
            json!({
                "workspacePath": "/remote/workspace",
                "workspaceIdentity": IDENTITY,
                "envelope": {
                    "commandId": "cmd-1",
                    "clientId": "client-1",
                    "sessionId": "fixture-session",
                    "type": "sendText",
                    "payload": { "firstInput": { "text": "hello" } },
                },
            }),
        )
        .await;
        let command_ack = loop {
            let (kind, id, payload) = next_frame(&mut socket, &mut frames).await;
            if id != 4 {
                continue;
            }
            assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{payload:?}");
            let Wire::Object(value) = payload else {
                panic!("command ack is an object");
            };
            break value;
        };

        let rust = json!({
            "mode": mode,
            "hello": normalize_v4_hello(hello),
            "ack": ack.expect("subscribe ack"),
            "frames": sort_v4_frames(event_frames),
            "commandAck": command_ack,
        });
        assert_eq!(rust, oracle, "path={path} mode={mode}");

        drop(socket);
        surface.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_window_controller_reads_and_mutates_the_ts_task_channel() {
    let surface = start_surface(Vec::new()).await;
    let (status, issued) = http_post(
        surface.addr,
        "/api/rpc-host-capability",
        &[(escode_host_rpc::http::HOST_SECRET_HEADER, &surface.secret)],
        &json!({
            "scope": {
                "sourceServerId": SOURCE,
                "profileId": PROFILE,
                "workspaceIdentity": IDENTITY,
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "scope 连接成功后签发 capability: {issued}");
    let capability = issued["capability"].as_str().expect("capability string");
    let mut socket = ws_connect(surface.addr, capability).await;
    let mut frames = FrameReader::new();

    let (kind, _, payload) = next_frame(&mut socket, &mut frames).await;
    assert_eq!(kind, session::RESPONSE_INITIALIZE);
    let Wire::Object(initialize) = payload else {
        panic!("Initialize payload is an object");
    };
    let names: Vec<&str> = initialize["channels"]
        .as_array()
        .expect("channel list")
        .iter()
        .filter_map(|name| name.as_str())
        .collect();
    assert!(
        names.contains(&TASK_CHANNEL),
        "fixture 广播了 escode-task: {names:?}"
    );

    // 先挂帧监听再订阅，快照按 TS 顺序在 ack 之前投递。
    send_listen(
        &mut socket,
        2,
        "window-controller",
        "onDynamicControllerFrame",
    )
    .await;
    send_call_arg(
        &mut socket,
        1,
        "window-controller",
        "subscribeControllerV4",
        json!({"topic": "controller/tasks-index"}),
    )
    .await;
    let mut snapshot = None;
    let (kind, payload) =
        read_until_promise(&mut socket, &mut frames, 1, &mut snapshot, &mut None).await;
    assert_eq!(
        kind,
        session::RESPONSE_PROMISE_SUCCESS,
        "订阅成功: {payload:?}"
    );
    let snapshot = match snapshot {
        Some(snapshot) => snapshot,
        None => next_snapshot(&mut socket, &mut frames).await,
    };
    let tasks = snapshot["payload"]["snapshot"]["tasks"]
        .as_array()
        .expect("snapshot tasks");
    let task = tasks
        .iter()
        .find(|row| row["address"]["taskId"] == json!("task-remote-1"))
        .expect("fixture task row");
    assert_eq!(task["address"]["sourceServerId"], json!(SOURCE));
    assert_eq!(task["address"]["workspaceIdentity"], json!(IDENTITY));
    assert_eq!(task["address"]["workspacePath"], json!("/remote/workspace"));
    assert_eq!(task["membership"]["pinned"], json!(false));
    let subscription_id = payload["ack"]["subscriptionId"]
        .as_str()
        .expect("subscription id")
        .to_owned();

    // pin：Host 转调远端写面，事件 + 强制刷新后 membership 与远端一致。
    send_call_arg(
        &mut socket,
        3,
        "window-controller",
        "mutateTask",
        json!({
            "address": {
                "taskId": "task-remote-1",
                "workspacePath": "/remote/workspace",
                "workspaceIdentity": IDENTITY,
                "sourceServerId": SOURCE,
            },
            "mutation": {"kind": "pin", "pinned": true},
        }),
    )
    .await;
    let mut delta = None;
    let (kind, payload) =
        read_until_promise(&mut socket, &mut frames, 3, &mut None, &mut delta).await;
    assert_eq!(
        kind,
        session::RESPONSE_PROMISE_SUCCESS,
        "pin 成功: {payload:?}"
    );
    assert_eq!(payload["taskId"], json!("task-remote-1"));
    let delta = match delta {
        Some(delta) => delta,
        None => next_delta(&mut socket, &mut frames).await,
    };
    assert!(
        delta["payload"]["deltas"]
            .as_array()
            .expect("deltas")
            .iter()
            .any(|op| op["op"] == json!("task.upserted")
                && op["task"]["address"]["taskId"] == json!("task-remote-1")
                && op["task"]["membership"]["pinned"] == json!(true)),
        "增量帧必须带更新后的 membership: {delta}"
    );

    send_call_arg(
        &mut socket,
        4,
        "window-controller",
        "resyncControllerV4",
        json!({"subscriptionId": subscription_id}),
    )
    .await;
    let mut resynced = None;
    let (kind, payload) =
        read_until_promise(&mut socket, &mut frames, 4, &mut resynced, &mut None).await;
    assert_eq!(
        kind,
        session::RESPONSE_PROMISE_SUCCESS,
        "resync: {payload:?}"
    );
    let resynced = match resynced {
        Some(snapshot) => snapshot,
        None => next_snapshot(&mut socket, &mut frames).await,
    };
    let task = resynced["payload"]["snapshot"]["tasks"]
        .as_array()
        .expect("resynced tasks")
        .iter()
        .find(|row| row["address"]["taskId"] == json!("task-remote-1"))
        .expect("resynced task row");
    assert_eq!(task["membership"]["pinned"], json!(true));
    assert_eq!(task["address"]["sourceServerId"], json!(SOURCE));

    // 其它 workspace 的地址不匹配唯一 source：写操作显式拒绝，不转调远端。
    send_call_arg(
        &mut socket,
        5,
        "window-controller",
        "mutateTask",
        json!({
            "address": {
                "taskId": "task-remote-1",
                "workspacePath": "/remote/other",
                "workspaceIdentity": "remote:server:55555555-5555-4555-8555-555555555555:/remote/other",
                "sourceServerId": "55555555-5555-4555-8555-555555555555",
            },
            "mutation": {"kind": "pin", "pinned": false},
        }),
    )
    .await;
    let (kind, payload) =
        read_until_promise(&mut socket, &mut frames, 5, &mut None, &mut None).await;
    assert_eq!(
        kind,
        session::RESPONSE_PROMISE_ERROR,
        "作用域不匹配: {payload:?}"
    );
    assert_eq!(payload["message"], json!("没有与任务地址匹配的 source"));

    drop(socket);
    surface.shutdown().await;
}

/// 验收 5：fixture 未注册 `escode-task` 时，远端 controller 返回显式错误，不回落本机 DB。
#[tokio::test(flavor = "multi_thread")]
async fn missing_task_channel_is_reported_instead_of_falling_back() {
    let surface = start_surface(vec![(
        "ESCODE_FIXTURE_WITHOUT_TASK".to_owned(),
        "1".to_owned(),
    )])
    .await;
    let (status, issued) = http_post(
        surface.addr,
        "/api/rpc-host-capability",
        &[(escode_host_rpc::http::HOST_SECRET_HEADER, &surface.secret)],
        &json!({
            "scope": {
                "sourceServerId": SOURCE,
                "profileId": PROFILE,
                "workspaceIdentity": IDENTITY,
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "连接成功即签发: {issued}");
    let capability = issued["capability"].as_str().expect("capability string");
    let mut socket = ws_connect(surface.addr, capability).await;
    let mut frames = FrameReader::new();

    let (kind, _, payload) = next_frame(&mut socket, &mut frames).await;
    assert_eq!(kind, session::RESPONSE_INITIALIZE);
    let Wire::Object(initialize) = payload else {
        panic!("Initialize payload is an object");
    };
    let names: Vec<&str> = initialize["channels"]
        .as_array()
        .expect("channel list")
        .iter()
        .filter_map(|name| name.as_str())
        .collect();
    assert!(
        !names.contains(&TASK_CHANNEL),
        "没有 escode-task: {names:?}"
    );

    send_call_arg(
        &mut socket,
        1,
        "window-controller",
        "listTaskList",
        json!({"kind": "active"}),
    )
    .await;
    let (kind, payload) =
        read_until_promise(&mut socket, &mut frames, 1, &mut None, &mut None).await;
    assert_eq!(
        kind,
        session::RESPONSE_PROMISE_ERROR,
        "缺失 channel: {payload:?}"
    );
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("escode-task.listTasks failed"),
        "错误必须来自远端调用而不是空列表: {message}"
    );

    drop(socket);
    surface.shutdown().await;
}
