//! `host-rpc` 端到端测试的共享辅助：真实 WebSocket + 手写 RPC 编解码 + 极简 HTTP 客户端。
//! 与 TS 客户端同布局（`serialize([type,id,channel,method])` + arg）。
#![allow(dead_code)] // 各测试文件只用到其中一个子集。

use std::time::Duration;

pub use escode_host_rpc::RpcSurface;
pub use escode_host_rpc::frame::FrameReader;
pub use escode_host_rpc::session;
pub use escode_host_rpc::wire::{Reader, Wire, Writer};
pub use futures_util::{SinkExt, StreamExt};
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub use tokio::net::TcpStream;
pub use tokio_tungstenite::tungstenite::Message;

pub type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

pub async fn start(home: &std::path::Path) -> RpcSurface {
    start_with_data_base_dir(home, home).await
}

/// `home` = 设置域根（homedir 语义）；`data_base_dir` = Main 提交的应用数据根（会话工作区用）。
pub async fn start_with_data_base_dir(
    home: &std::path::Path,
    data_base_dir: &std::path::Path,
) -> RpcSurface {
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
                // 会话夹具经 RPC 的 `config` 传入 `--config` 同格式的模型配置（组合根同法）。
                config: workspace.config.clone().map(std::path::PathBuf::from),
            }
        }
    });
    let channels = escode_host_rpc::domains::channels(
        home.to_path_buf(),
        Some(data_base_dir.to_path_buf()),
        instances.clone(),
        options,
    );
    escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, Some(instances))
        .await
        .expect("rpc surface starts")
}

/// 极简 HTTP 客户端（GET/POST + 可选头），返回 (status, json body)。
pub async fn http_request(
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

pub async fn connect(addr: std::net::SocketAddr, path: &str, capability: Option<&str>) -> Socket {
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

/// 合法调用方的连接方式（Main 同法）：带进程密钥申请一次性 capability，再连 `/ws/host`。
pub async fn open(surface: &RpcSurface) -> Socket {
    let capability = issue_capability(surface).await;
    connect(surface.addr, "/ws/host", Some(&capability)).await
}

/// 带进程密钥申请一次性 capability。
pub async fn issue_capability(surface: &RpcSurface) -> String {
    let (status, issued) = http_request(
        surface.addr,
        "POST",
        "/api/rpc-host-capability",
        &[(escode_host_rpc::http::HOST_SECRET_HEADER, &surface.secret)],
        None,
    )
    .await;
    assert_eq!(status, 200, "capability issue failed: {issued}");
    issued["capability"]
        .as_str()
        .expect("capability string")
        .to_owned()
}

/// 进程密钥头（HTTP 路由用）。
pub fn secret_header(surface: &RpcSurface) -> [(&'static str, &str); 1] {
    [(
        escode_host_rpc::http::HOST_SECRET_HEADER,
        surface.secret.as_str(),
    )]
}

/// 读一条 RPC 响应（跳过 Initialize）。
pub async fn read_response(socket: &mut Socket, expect_id: i64) -> Wire {
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
pub async fn call(socket: &mut Socket, id: i64, channel: &str, method: &str, arg: &Wire) {
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

/// 建库夹具：Rust Host **不负责建库**（`TasksIndex::open` 对缺失/账本不符一律结构化拒绝，
/// 见子 spec §3），dev 开关下由 TS Host 先经 `prepareTasksIndexStorage` 建库。
pub fn prepare_index(data_base_dir: &std::path::Path) {
    let path = data_base_dir
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    for statement in escode_host_services::tasks_index::schema::MIGRATED_DDL {
        connection.execute_batch(statement).unwrap();
    }
    for (id, checksum) in escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS {
        connection
            .execute(
                "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
                rusqlite::params![id, checksum],
            )
            .unwrap();
    }
}
