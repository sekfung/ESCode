//! HTTP/WS 入口：对齐 `packages/escode-server-cli/src/server-core/http.ts` 的本机契约。
//!
//! - `GET /api/server-info`：`{ serverId, version, protocolVersion, authRequired, workspaces, capabilities }`；
//! - `POST /api/rpc-host-capability`：签发一次性 capability（TTL），供 `/ws/host` 校验；
//! - `GET /ws`：`web-remote-replayable` 语义（回环免鉴权）；
//! - `GET /ws/host`：`desktop-continuous` 语义，需 `x-escode-rpc-host-capability` 头。
//!
//! 只允许回环监听（与 TS 的 fail-closed 一致：非回环必须先进鉴权）。
//! HTTP 解析是最小子集（请求行 + 头 + 可选 body），响应一律 `Connection: close`。

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// capability 头名（TS `ESCODE_RPC_HOST_CAPABILITY_HEADER`）。
pub const CAPABILITY_HEADER: &str = "x-escode-rpc-host-capability";

/// 协议版本（与 TS `SERVER_REMOTE_PROTOCOL_VERSION` 对齐的口径位；u2 起由 Main 校验）。
pub const PROTOCOL_VERSION: u32 = 1;

/// 请求头最大字节数（含请求行）。
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// 已解析的请求头。
#[derive(Debug)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
}

impl RequestHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn is_websocket_upgrade(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
    }
}

/// 从连接里窥探请求头（`peek` 不消费字节，便于随后交给 WebSocket 握手）。
pub async fn peek_head(stream: &TcpStream) -> Result<Option<RequestHead>> {
    let mut buffer = vec![0_u8; MAX_HEAD_BYTES];
    loop {
        let read = stream.peek(&mut buffer).await.context("HTTP peek failed")?;
        if read == 0 {
            return Ok(None);
        }
        if let Some(end) = find_head_end(&buffer[..read]) {
            let text = String::from_utf8_lossy(&buffer[..end]);
            return Ok(Some(parse_head(&text)?));
        }
        anyhow::ensure!(
            read < MAX_HEAD_BYTES,
            "HTTP request head exceeds size limit"
        );
        // 头还没读完：给内核一点时间把剩余字节送到。
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}

/// 读取请求头（消费字节；用于普通 HTTP 请求）。
pub async fn read_head(stream: &mut TcpStream) -> Result<Option<RequestHead>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.context("HTTP read failed")?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = find_head_end(&buffer) {
            let text = String::from_utf8_lossy(&buffer[..end]);
            return Ok(Some(parse_head(&text)?));
        }
        anyhow::ensure!(
            buffer.len() < MAX_HEAD_BYTES,
            "HTTP request head exceeds size limit"
        );
    }
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_head(text: &str) -> Result<RequestHead> {
    let mut lines = text.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("missing method")?.to_owned();
    let path = parts.next().context("missing path")?.to_owned();
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Ok(RequestHead {
        method,
        path,
        headers,
    })
}

/// 写一个 JSON 响应（`Connection: close`）。
pub async fn respond_json(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &serde_json::Value,
) -> Result<()> {
    let payload = serde_json::to_vec(body)?;
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}

/// 回环地址校验（与 TS `isLoopbackHost` 同：非回环 fail-closed）。
pub fn ensure_loopback(bind: SocketAddr) -> Result<()> {
    let loopback = match bind.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    };
    anyhow::ensure!(
        loopback,
        "Non-loopback host {} requires authentication before the server can listen",
        bind
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request_head_case_insensitively() {
        let head = parse_head(
            "GET /ws/host HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: WebSocket\r\nX-ESCODE-RPC-Host-Capability: abc\r\n",
        )
        .expect("parses");
        assert_eq!(head.method, "GET");
        assert_eq!(head.path, "/ws/host");
        assert!(head.is_websocket_upgrade());
        assert_eq!(head.header("x-escode-rpc-host-capability"), Some("abc"));
    }

    #[test]
    fn rejects_non_loopback_binds() {
        assert!(ensure_loopback("127.0.0.1:0".parse().unwrap()).is_ok());
        assert!(ensure_loopback("[::1]:0".parse().unwrap()).is_ok());
        let error = ensure_loopback("0.0.0.0:8080".parse().unwrap()).expect_err("非回环必须失败");
        assert!(error.to_string().contains("authentication"));
    }
}
