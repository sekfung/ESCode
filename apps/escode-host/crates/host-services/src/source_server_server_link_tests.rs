//! S5-f-3：server 直连走完整 establish（HTTP 探测 → 身份判定 → WebSocket → 握手记录）。
//!
//! 用同一端口上的最小 fixture 服务端同时提供 `/api/server-info`、`/api/rpc-host-capability`
//! 与 `/ws/host`（`accept_async` 后立即广播 Initialize），不依赖 Node。

use std::sync::Arc;
use std::time::Duration;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_core::rpc_wire::encode_regular;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::source_server::SourceServerRegistry;
use crate::source_server_client::RESPONSE_INITIALIZE;
use crate::source_server_connections::{ConnectionsOptions, SourceServerConnections};
use crate::source_server_exec::{ResolvedTransport, ServerConnect};

const PROFILE: &str = "00000000-0000-4000-8000-000000000042";
const SERVER: &str = "srv-server-link";

fn initialize_frame() -> Vec<u8> {
    let mut payload =
        WireValue::Array(vec![WireValue::Int(RESPONSE_INITIALIZE), WireValue::Int(0)]).encode();
    WireValue::from_json(&json!({"channels": ["escode-agent"]})).encode_into(&mut payload);
    encode_regular(&payload)
}

fn server_info() -> String {
    json!({
        "serverId": SERVER,
        "name": "fixture",
        "version": "3.16.2",
        "protocolVersion": 1,
        "authRequired": false,
        "workspaces": [
            {"path": "/home/dev/one", "label": "one", "workspaceIdentity": "remote:one"},
            {"path": "/home/dev/two"},
        ],
        "capabilities": {"desktopContinuous": true, "websocketRpc": true},
    })
    .to_string()
}

async fn peek_request_head(stream: &TcpStream) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    loop {
        match stream.peek(&mut buffer).await {
            Ok(n) if n > 0 => {
                if buffer[..n].windows(4).any(|window| window == b"\r\n\r\n") {
                    return Some(String::from_utf8_lossy(&buffer[..n]).to_string());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            _ => return None,
        }
    }
}

async fn serve_http(mut stream: TcpStream, body: String) {
    let mut discard = vec![0u8; 4096];
    let _ = stream.read(&mut discard).await;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn spawn_fixture() -> (std::net::SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Some(head) = peek_request_head(&stream).await else {
                    return;
                };
                if head.starts_with("GET /ws/host") {
                    let mut socket = tokio_tungstenite::accept_async(stream)
                        .await
                        .expect("ws handshake");
                    let _ = socket.send(Message::binary(initialize_frame())).await;
                    // 保持链路直到客户端关闭，模拟 desktop-continuous 长连接。
                    while let Some(message) = socket.next().await {
                        if message.is_err() {
                            break;
                        }
                    }
                    return;
                }
                let body = if head.starts_with("GET /api/server-info") {
                    server_info()
                } else {
                    json!({"capability": "ticket-1", "expiresAt": 1_700_000_000_000i64}).to_string()
                };
                serve_http(stream, body).await;
            });
        }
    });
    (address, task)
}

/// 握手记录会写回 settings，因此 registry 必须带真实 settings home。
fn registry(home: &std::path::Path, url: &str) -> Arc<SourceServerRegistry> {
    let path = crate::settings::settings_path(home);
    std::fs::create_dir_all(path.parent().expect("settings dir")).expect("settings dir");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "sourceServers": [],
            "connectionProfiles": [{
                "profileId": PROFILE,
                "transport": "server",
                "target": {"kind": "server", "url": url},
                "createdAt": 1_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("settings json"),
    )
    .expect("settings write");
    Arc::new(SourceServerRegistry::load(home))
}

fn connections(registry: Arc<SourceServerRegistry>, url: String) -> SourceServerConnections {
    let options = ConnectionsOptions::new("3.15.2", move |_| {
        let url = url.clone();
        Box::pin(async move {
            Ok(ResolvedTransport::server(ServerConnect {
                url,
                token: None,
            }))
        })
    });
    SourceServerConnections::new(registry, options)
}

#[tokio::test]
async fn server_link_establishes_and_binds_identity() {
    let home = tempfile::tempdir().expect("home");
    let (address, fixture) = spawn_fixture().await;
    let url = format!("http://{address}");
    // 目标可以带尾部斜杠：端点解析负责规范化。
    let registry = registry(home.path(), &format!("{url}/"));
    let connections = connections(Arc::clone(&registry), url.clone());

    connections.connect(PROFILE).await.expect("connect");

    let state = registry.connection_state(PROFILE).expect("state");
    assert_eq!(state.state, "online");
    assert_eq!(state.source_server_id.as_deref(), Some(SERVER));
    assert_eq!(state.current_version.as_deref(), Some("3.16.2"));

    let profile = registry.profile(PROFILE).expect("profile");
    assert_eq!(profile.source_server_id.as_deref(), Some(SERVER));

    // workspaces 由 server-info 投影：路径、label、identity 都要保留。
    let server = registry.source_server(SERVER).expect("server record");
    let paths: Vec<&str> = server
        .declared_workspaces
        .iter()
        .map(|workspace| workspace.workspace_path.as_str())
        .collect();
    assert_eq!(paths, vec!["/home/dev/one", "/home/dev/two"]);
    // 服务端自报的 identity 只作候选：绑定前统一改写成 canonical 形式（与 S4-j 同一规则）。
    assert_eq!(
        server.declared_workspaces[0].workspace_identity.as_str(),
        "remote:server:srv-server-link:/home/dev/one"
    );
    assert_eq!(
        server.declared_workspaces[1].workspace_identity.as_str(),
        "remote:server:srv-server-link:/home/dev/two"
    );

    assert_eq!(connections.connected_count(), 1);
    connections.disconnect(PROFILE).await;
    fixture.abort();
}

#[tokio::test]
async fn server_link_reuses_the_existing_connection() {
    // 直接调用连接打开阶段：缺少探测结果必须显式失败，不能静默建链。
    let home = tempfile::tempdir().expect("home");
    let (address, fixture) = spawn_fixture().await;
    let url = format!("http://{address}");
    let registry = registry(home.path(), &url);
    let connections = connections(Arc::clone(&registry), url);
    let result = connections.connect(PROFILE).await;
    // 正常路径成功；这里验证的是同一连接只建一条链路，重复 connect 复用而不是重生。
    result.expect("connect");
    connections.connect(PROFILE).await.expect("reuse");
    assert_eq!(connections.connected_count(), 1);
    connections.disconnect(PROFILE).await;
    fixture.abort();
}
