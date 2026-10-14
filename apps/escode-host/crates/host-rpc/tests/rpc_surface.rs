//! RPC 服务面端到端：真实 WebSocket 上跑完整 `@escode/rpc` 协议。
//!
//! 覆盖：HTTP 契约（`/api/server-info`、一次性 capability、`/ws/host` 鉴权 fail-closed）、
//! `Initialize`、`setting.get/update` 往返、未迁移 channel 的显式 `NotImplemented`。
use std::time::Duration;

use escode_host_rpc::frame::FrameReader;
use escode_host_rpc::wire::{Reader, Wire, Writer};
use escode_host_rpc::{Channels, RpcSurface, session};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

async fn start(home: &std::path::Path) -> RpcSurface {
    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory = std::sync::Arc::new({
        let home = home.to_path_buf();
        move |workspace: &escode_host_control::WorkspaceDescriptor| {
            let mut env = std::collections::BTreeMap::new();
            env.insert("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned());
            escode_cli_rust::embed::RuntimeOptions {
                home: home.clone(),
                env,
                argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: workspace.data_dir.clone().map(std::path::PathBuf::from),
                config: None,
            }
        }
    });
    let channels =
        escode_host_rpc::domains::channels(home.to_path_buf(), instances.clone(), options);
    escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, Some(instances))
        .await
        .expect("rpc surface starts")
}

/// 极简 HTTP 客户端（GET/POST + 可选头），返回 (status, json body)。
async fn http_request(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut stream = TcpStream::connect(addr).await.expect("http connect");
    let mut request = format!("{method} {path} HTTP/1.1\r\nhost: {addr}\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    let payload = body.unwrap_or("");
    request.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
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
    let json = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn connect(addr: std::net::SocketAddr, path: &str, capability: Option<&str>) -> Socket {
    let mut builder = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://{addr}{path}"))
        .header("host", addr.to_string())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    if let Some(capability) = capability {
        builder = builder.header(escode_host_rpc::http::CAPABILITY_HEADER, capability);
    }
    let request = builder.body(()).expect("request builds");
    let stream = TcpStream::connect(addr).await.expect("ws connect");
    let (socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("ws upgrade");
    socket
}

/// 读一条 RPC 响应（跳过 Initialize）。
async fn read_response(socket: &mut Socket, expect_id: i64) -> Wire {
    let mut frames = FrameReader::new();
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
        while let Some(body) = frames.next_frame().expect("frame parses") {
            let mut reader = Reader::new(&body);
            let Wire::Array(header) = reader.value().expect("header") else {
                panic!("response header must be an array")
            };
            let kind = header[0].as_int().expect("kind");
            if kind == session::RESPONSE_INITIALIZE {
                continue;
            }
            if header.get(1).and_then(Wire::as_int) == Some(expect_id) {
                return reader.value().expect("payload");
            }
        }
    }
}

/// 发送一条 Promise 请求（与 TS 客户端同布局）。
async fn call(socket: &mut Socket, id: i64, channel: &str, method: &str, arg: &Wire) {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_PROMISE),
        Wire::Int(id),
        Wire::Str(channel.to_owned()),
        Wire::Str(method.to_owned()),
    ]));
    writer.value(arg);
    let frame = escode_host_rpc::frame::frame(&writer.into_bytes());
    socket
        .send(Message::Binary(frame.into()))
        .await
        .expect("request sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn server_info_and_capability_gate_the_websocket() {
    let dir = tempfile::tempdir().unwrap();
    let surface = start(dir.path()).await;

    let (status, info) = http_request(surface.addr, "GET", "/api/server-info", &[], None).await;
    assert_eq!(status, 200);
    assert_eq!(info["capabilities"]["websocketRpc"], true);
    assert_eq!(info["authRequired"], false);
    assert!(
        info["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "setting")
    );

    // 无 capability 的 /ws/host 必须被拒绝（fail-closed，与 TS 401 同义）。
    let stream = TcpStream::connect(surface.addr).await.unwrap();
    let mut stream = stream;
    stream
        .write_all(
            format!(
                "GET /ws/host HTTP/1.1\r\nhost: {}\r\nconnection: Upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
                surface.addr
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.contains("401") || text.contains("400"),
        "无 capability 必须拒绝: {}",
        &text[..text.len().min(120)]
    );

    // 带 capability 的连接成功。
    let (_, issued) = http_request(
        surface.addr,
        "POST",
        "/api/rpc-host-capability",
        &[],
        Some("{}"),
    )
    .await;
    let capability = issued["capability"]
        .as_str()
        .expect("capability")
        .to_owned();
    let mut socket = connect(surface.addr, "/ws/host", Some(&capability)).await;
    call(&mut socket, 1, "setting", "get", &Wire::Undefined).await;
    let response = read_response(&mut socket, 1).await;
    assert!(
        response.into_object().is_some(),
        "capability 连接应能调用 setting.get"
    );

    // capability 是一次性的：同一 token 再用必须失败。
    let stream = TcpStream::connect(surface.addr).await.unwrap();
    let mut stream = stream;
    stream
        .write_all(
            format!(
                "GET /ws/host HTTP/1.1\r\nhost: {}\r\nconnection: Upgrade\r\nupgrade: websocket\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n{}: {capability}\r\n\r\n",
                surface.addr,
                escode_host_rpc::http::CAPABILITY_HEADER
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.contains("401") || text.contains("400"),
        "一次性 token 复用必须拒绝"
    );

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_round_trip_and_unimplemented_channels_report_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let surface = start(dir.path()).await;
    let mut socket = connect(surface.addr, "/ws", None).await;

    // 1. setting.update → 落盘；setting.get 读回。
    call(
        &mut socket,
        7,
        "setting",
        "update",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"locale": "zh-CN"}))]),
    )
    .await;
    assert_eq!(read_response(&mut socket, 7).await, Wire::Undefined);
    call(&mut socket, 8, "setting", "get", &Wire::Undefined).await;
    let settings = read_response(&mut socket, 8)
        .await
        .into_object()
        .expect("object");
    assert_eq!(settings["locale"], "zh-CN");

    // 2. 未迁移 channel：显式 NotImplemented（含已实现清单，Renderer 可诊断）。
    call(
        &mut socket,
        9,
        "workspace",
        "query",
        &Wire::Object(serde_json::json!({})),
    )
    .await;
    let error = read_response(&mut socket, 9)
        .await
        .into_object()
        .expect("error object");
    assert_eq!(error["name"], "NotImplemented");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("workspace"), "{message}");
    assert!(message.contains("setting"), "{message}");

    // 3. 事件面同样显式缺口。
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_EVENT_LISTEN),
        Wire::Int(10),
        Wire::Str("task".to_owned()),
        Wire::Str("onDidChange".to_owned()),
    ]));
    writer.value(&Wire::Undefined);
    socket
        .send(Message::Binary(
            escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
        ))
        .await
        .unwrap();
    let error = read_response(&mut socket, 10)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "NotImplemented");

    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn non_loopback_bind_is_refused() {
    let channels: Channels = escode_host_rpc::domains::channels(
        std::path::PathBuf::from("/tmp"),
        escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded),
        std::sync::Arc::new(|workspace: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: std::path::PathBuf::from("/tmp"),
                env: std::collections::BTreeMap::new(),
                argv: vec![],
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: None,
                config: None,
            }
        }),
    );
    let result = escode_host_rpc::serve("0.0.0.0:0".parse().unwrap(), channels, None).await;
    let error = match result {
        Ok(_) => panic!("非回环必须 fail-closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("authentication"));
}

#[tokio::test(flavor = "multi_thread")]
async fn session_list_forwards_to_the_workspace_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let surface = start(&home).await;
    let mut socket = connect(surface.addr, "/ws", None).await;

    // escode-session.listSessions：Host 把它转成 runtime 的 session/list（TS Host 同构）。
    call(
        &mut socket,
        21,
        "escode-session",
        "listSessions",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "workspaceIdentity": workspace.to_string_lossy(),
        }))]),
    )
    .await;
    let sessions = read_response(&mut socket, 21)
        .await
        .into_object()
        .expect("array");
    assert_eq!(sessions, serde_json::json!([]), "新 workspace 没有会话");

    // 实例已按 workspace 建好（/api/server-info 报告 workspaces）。
    let (_, info) = http_request(surface.addr, "GET", "/api/server-info", &[], None).await;
    let expected_workspace = workspace.to_string_lossy().into_owned();
    assert!(
        info["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str() == Some(expected_workspace.as_str())),
        "server-info 应报告已建实例: {info}"
    );
    assert!(
        info["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "escode-session"),
        "能力清单应含 escode-session: {info}"
    );

    // 未迁移方法：显式缺口。
    call(
        &mut socket,
        22,
        "escode-session",
        "createSession",
        &Wire::Array(vec![Wire::Object(serde_json::json!({}))]),
    )
    .await;
    let error = read_response(&mut socket, 22)
        .await
        .into_object()
        .expect("error");
    assert_eq!(error["name"], "NotImplemented");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("escode-session.createSession")
    );

    surface.shutdown().await;
}
