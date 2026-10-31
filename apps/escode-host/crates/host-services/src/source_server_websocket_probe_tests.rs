use std::net::SocketAddr;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::{HOST_CAPABILITY_HEADER, probe_server};
use crate::source_server_endpoints::ServerRemoteEndpoints;

#[derive(Debug, Default)]
struct CapturedRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn endpoints(base: SocketAddr) -> ServerRemoteEndpoints {
    ServerRemoteEndpoints {
        info_url: format!("http://{base}/api/server-info"),
        ws_url: format!("ws://{base}/ws"),
        host_capability_url: format!("http://{base}/api/rpc-host-capability"),
        host_ws_url: format!("ws://{base}/ws/host"),
    }
}

fn server_info() -> Value {
    json!({
        "serverId": "srv-1",
        "version": "3.16.2",
        "protocolVersion": 1,
        "authRequired": true,
        "workspaces": [{"path": "/w"}],
        "capabilities": {"desktopContinuous": true, "websocketRpc": true},
    })
}

/// 最小 HTTP/1.1 服务端：按顺序回放 `responses`，记录每个请求的方法/路径/头。
async fn finish(server: JoinHandle<Vec<CapturedRequest>>) -> Vec<CapturedRequest> {
    tokio::time::timeout(std::time::Duration::from_secs(10), server)
        .await
        .expect("test server did not finish")
        .expect("server")
}

async fn spawn_http(
    responses: Vec<(u16, String)>,
) -> (SocketAddr, JoinHandle<Vec<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.expect("read");
                assert!(read > 0, "client closed before headers");
                buffer.extend_from_slice(&chunk[..read]);
            }
            let head = String::from_utf8_lossy(&buffer).to_string();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or_default().to_owned();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let target = parts.next().unwrap_or_default().to_owned();
            let headers = lines
                .filter(|line| !line.is_empty())
                .filter_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    Some((key.to_ascii_lowercase(), value.trim().to_owned()))
                })
                .collect();
            captured.push(CapturedRequest {
                method,
                target,
                headers,
            });
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.expect("write");
            stream.flush().await.expect("flush");
        }
        captured
    });
    (address, task)
}

#[tokio::test]
async fn probe_sends_bearer_header_and_token_query() {
    let capability =
        json!({"capability": "ticket-1", "expiresAt": 1_700_000_000_000i64}).to_string();
    let (address, server) =
        spawn_http(vec![(200, server_info().to_string()), (200, capability)]).await;
    let probe = probe_server(&endpoints(address), Some(" secret "))
        .await
        .expect("probe");
    assert_eq!(probe.info.server_id, "srv-1");
    assert_eq!(probe.capability, "ticket-1");
    let captured = finish(server).await;
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].method, "GET");
    assert_eq!(captured[0].target, "/api/server-info?token=secret");
    assert_eq!(captured[0].header("authorization"), Some("Bearer secret"));
    // POST 也要带同一凭据：capability 是 trusted-host 升级的前置条件。
    assert_eq!(captured[1].method, "POST");
    assert_eq!(captured[1].target, "/api/rpc-host-capability?token=secret");
    assert_eq!(captured[1].header("authorization"), Some("Bearer secret"));
    // Host 侧不发送 capability 头去申请 capability，避免自我提权。
    assert_eq!(captured[1].header(HOST_CAPABILITY_HEADER), None);
}

#[tokio::test]
async fn probe_without_token_sends_no_credentials() {
    let capability = json!({"capability": "ticket-1", "expiresAt": 1}).to_string();
    let (address, server) =
        spawn_http(vec![(200, server_info().to_string()), (200, capability)]).await;
    probe_server(&endpoints(address), None)
        .await
        .expect("probe");
    let captured = finish(server).await;
    assert_eq!(captured[0].target, "/api/server-info");
    assert_eq!(captured[0].header("authorization"), None);
    assert_eq!(captured[1].header("authorization"), None);
}

#[tokio::test]
async fn probe_fails_explicitly_on_http_error() {
    let (address, server) =
        spawn_http(vec![(401, "{\"error\":\"Unauthorized\"}".to_owned())]).await;
    let error = probe_server(&endpoints(address), None)
        .await
        .expect_err("unauthorized");
    finish(server).await;
    assert!(format!("{error:#}").contains("HTTP 401"), "{error:#}");
}

#[tokio::test]
async fn probe_rejects_incompatible_protocol() {
    let mut info = server_info();
    info["protocolVersion"] = json!(2);
    // server-info 解析失败即中止，不会再申请 capability。
    let (address, server) = spawn_http(vec![(200, info.to_string())]).await;
    let error = probe_server(&endpoints(address), None)
        .await
        .expect_err("protocol");
    finish(server).await;
    assert!(
        format!("{error:#}").contains("source-server-incompatible"),
        "{error:#}"
    );
}

#[tokio::test]
async fn probe_rejects_invalid_capability_body() {
    let (address, server) = spawn_http(vec![
        (200, server_info().to_string()),
        (200, "{}".to_owned()),
    ])
    .await;
    let error = probe_server(&endpoints(address), None)
        .await
        .expect_err("capability");
    finish(server).await;
    assert!(
        format!("{error:#}").contains("invalid host capability"),
        "{error:#}"
    );
}

#[tokio::test]
async fn probe_reports_non_json_bodies() {
    let (address, server) = spawn_http(vec![(200, "not json".to_owned())]).await;
    let error = probe_server(&endpoints(address), None)
        .await
        .expect_err("json");
    finish(server).await;
    assert!(format!("{error:#}").contains("not JSON"), "{error:#}");
}
