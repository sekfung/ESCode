use std::time::Duration;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_core::rpc_wire::{FrameDecoder, encode_regular};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use super::{HOST_CAPABILITY_HEADER, bridge, open_websocket};
use crate::source_server_client::{REQUEST_PROMISE, RESPONSE_INITIALIZE};
use crate::source_server_connection::ConnectionInfo;
use crate::source_server_endpoints::ServerRemoteEndpoints;

fn info() -> ConnectionInfo {
    ConnectionInfo {
        version: "3.16.2".to_owned(),
        compatibility: None,
        platform: None,
        arch: None,
        pid: None,
    }
}

fn initialize_frame() -> Vec<u8> {
    let mut payload =
        WireValue::Array(vec![WireValue::Int(RESPONSE_INITIALIZE), WireValue::Int(0)]).encode();
    WireValue::from_json(&json!({"channels": ["escode-agent"]})).encode_into(&mut payload);
    encode_regular(&payload)
}

fn frame_headers(frame: &[u8]) -> Vec<WireValue> {
    let mut decoder = FrameDecoder::new();
    decoder.push(frame);
    let payload = decoder.next_frame().expect("decode").expect("frame");
    match WireValue::decode(&payload).expect("wire value").0 {
        WireValue::Array(items) => items,
        other => panic!("unexpected header {other:?}"),
    }
}

#[tokio::test]
async fn forwards_frames_and_reports_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake");
        // 与 escode-server 一致：连接建立后主动广播 Initialize。
        socket
            .send(Message::binary(initialize_frame()))
            .await
            .expect("initialize");
        let frame = loop {
            match socket.next().await {
                Some(Ok(Message::Binary(bytes))) => break bytes.to_vec(),
                Some(Ok(_)) => continue,
                other => panic!("unexpected client message {other:?}"),
            }
        };
        socket.close(None).await.expect("close");
        frame
    });

    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws/host"))
        .await
        .expect("connect");
    let connection = bridge(socket, info());
    connection
        .client
        .wait_initialized()
        .await
        .expect("initialize");
    assert_eq!(connection.info.version, "3.16.2");
    assert_eq!(connection.info.platform, None);

    let client = std::sync::Arc::clone(&connection.client);
    let call = tokio::spawn(async move {
        client
            .call(
                "escode-agent",
                "ping",
                Some(WireValue::from_json(&json!({}))),
            )
            .await
    });
    let frame = server.await.expect("server");
    let headers = frame_headers(&frame);
    assert_eq!(headers.first(), Some(&WireValue::Int(REQUEST_PROMISE)));

    // 服务端关闭后链路必须置位 closed，并把挂起请求收敛成 ConnectionClosed。
    let mut closed = connection.closed();
    tokio::time::timeout(Duration::from_secs(5), closed.changed())
        .await
        .expect("closed signal")
        .expect("watch open");
    assert!(*closed.borrow());
    let error = call.await.expect("join").expect_err("call must fail");
    assert!(
        format!("{error:#}").contains("ConnectionClosed"),
        "{error:#}"
    );
}

// tungstenite 的握手回调签名固定返回大型 `ErrorResponse`；这里只在测试里读取请求头。
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn websocket_upgrade_carries_token_and_capability() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let captured: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let server_capture = std::sync::Arc::clone(&captured);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut socket = tokio_tungstenite::accept_hdr_async(
            stream,
            move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                  response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let mut headers = server_capture.lock().expect("lock");
                for (name, value) in request.headers() {
                    headers.push((
                        name.as_str().to_owned(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    ));
                }
                headers.push(("request-target".to_owned(), request.uri().to_string()));
                Ok(response)
            },
        )
        .await
        .expect("handshake");
        socket.close(None).await.ok();
    });
    let endpoints = ServerRemoteEndpoints {
        info_url: format!("http://{address}/api/server-info"),
        ws_url: format!("ws://{address}/ws"),
        host_capability_url: format!("http://{address}/api/rpc-host-capability"),
        host_ws_url: format!("ws://{address}/ws/host"),
    };
    let socket = open_websocket(&endpoints, Some("secret"), "ticket-1")
        .await
        .expect("upgrade");
    drop(socket);
    server.await.expect("server");
    let captured = captured.lock().expect("lock");
    let header = |name: &str| {
        captured
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    // 与 TS `connectNodeWebSocket` 一致：token 同时在 query 与 authorization，capability 只走 header。
    assert!(
        header("request-target").is_some_and(|target| target.contains("token=secret")),
        "{captured:?}"
    );
    assert_eq!(header("authorization"), Some("Bearer secret"));
    assert_eq!(header(HOST_CAPABILITY_HEADER), Some("ticket-1"));
}
