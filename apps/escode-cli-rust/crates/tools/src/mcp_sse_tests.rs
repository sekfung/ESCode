use super::*;
use rmcp::transport::Transport as _;
use std::collections::HashMap;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[test]
fn decoder_tracks_id_retry_and_split_frames() {
    let mut decoder = Decoder::default();
    assert!(
        decoder
            .push(b"id: 7\nretry: 25\nevent: message\ndata: {\"a\":1}")
            .unwrap()
            .is_empty()
    );
    assert_eq!(decoder.take_retry(), Some(25));
    let events = decoder.push(b"\n\nevent: endpoint\ndata: /m\n\n").unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, "message");
    assert_eq!(events[0].data, "{\"a\":1}");
    assert_eq!(events[0].id.as_deref(), Some("7"));
    assert_eq!(events[1].kind, "endpoint");
    assert_eq!(events[1].data, "/m");
    assert_eq!(events[1].id, None);
}

#[test]
fn empty_id_clears_last_event_id() {
    let mut decoder = Decoder::default();
    let events = decoder.push(b"id:\ndata: {}\n\n").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id.as_deref(), Some(""));
}

#[tokio::test]
async fn reconnects_with_last_event_id_and_updates_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (reconnected_tx, reconnected_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        // 第一次 GET：下发 endpoint 和 id=1，然后断开流。
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let (method, path, _) = read_head(&mut read).await;
        assert_eq!((method.as_str(), path.as_str()), ("GET", "/sse"));
        write
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
                      event: endpoint\ndata: /messages\n\n\
                      id: 1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
                )
                .await
                .unwrap();
        write.flush().await.unwrap();
        drop(write);

        // 重连 GET：必须带 Last-Event-ID: 1；下发新 endpoint 与 id=2。
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        let (method, path, headers) = read_head(&mut read).await;
        assert_eq!((method.as_str(), path.as_str()), ("GET", "/sse"));
        assert_eq!(headers.get("last-event-id").map(String::as_str), Some("1"));
        write
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
                      event: endpoint\ndata: /messages2\n\n\
                      id: 2\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n",
                )
                .await
                .unwrap();
        write.flush().await.unwrap();
        let _ = reconnected_tx.send(());

        // POST 必须使用重连后的 endpoint，且只发一次。
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut post_write) = socket.into_split();
        let mut read = BufReader::new(read);
        let (method, path, headers) = read_head(&mut read).await;
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/messages2"));
        let _ = read_body(&mut read, headers.get("content-length")).await;
        post_write
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        write
            .write_all(
                b"id: 3\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}\n\n",
            )
            .await
            .unwrap();
        write.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });

    let url = format!("http://{address}/sse");
    // 出口策略统一走 crates/net（clippy 禁止在业务 crate 就地构造 reqwest 客户端），
    // 测试因此也覆盖生产的代理/CA 单入口。
    let client = crate::mcp_http::client(&url).await.unwrap();
    let config = Server::parse(
        "remote",
        serde_json::json!({"type": "sse", "url": url}),
        Path::new("."),
    )
    .unwrap();
    let stop = CancellationToken::new();
    let mut transport = Transport::open(
        &config,
        client,
        None,
        &stop,
        super::super::mcp_raw_capture::Responses::default(),
    )
    .await
    .unwrap();

    let first = tokio::time::timeout(Duration::from_secs(5), transport.receive())
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_value(first).unwrap()["id"] == 1);
    let second = tokio::time::timeout(Duration::from_secs(5), transport.receive())
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_value(second).unwrap()["id"] == 2);
    reconnected_rx.await.unwrap();

    let request: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": {}
    }))
    .unwrap();
    transport.send(request).await.unwrap();
    let third = tokio::time::timeout(Duration::from_secs(5), transport.receive())
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_value(third).unwrap()["id"] == 3);

    transport.close().await.unwrap();
    server.abort();
}

async fn read_head<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> (String, String, HashMap<String, String>) {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap().to_string();
    let path = parts.next().unwrap().to_string();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    (method, path, headers)
}

async fn read_body<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    length: Option<&String>,
) -> Vec<u8> {
    let length = length.and_then(|value| value.parse().ok()).unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    body
}
