//! `kind: "server"` 直连链路：HTTP 探测 + WebSocket 帧面（S5-f-2，§12.6.1/§12.6.4）。
//!
//! 与 stdio 分支的差异只在链路：远端命令换成 `GET /api/server-info` +
//! `POST /api/rpc-host-capability` + `ws /ws/host`，帧面仍是 `escode_host_core::rpc_wire` 的
//! Regular 帧，之上复用同一个 [`RemoteChannelClient`]。
//!
//! token 只进请求头与 query，绝不进日志或错误消息的 URL 部分；capability 一次性使用，
//! 每次重连重新申请。

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use escode_host_control::logging;
use escode_host_core::rpc_wire::FrameDecoder;

use crate::source_server_client::RemoteChannelClient;
use crate::source_server_connection::{ConnectionInfo, RemoteConnection};
use crate::source_server_endpoints::{
    ServerRemoteEndpoints, authenticated_url, resolve_server_remote_endpoints,
};
use crate::source_server_info::{ServerRemoteInfo, parse_host_capability, parse_server_info};

/// 与 TS `ESCODE_RPC_HOST_CAPABILITY_HEADER` 一致。
pub const HOST_CAPABILITY_HEADER: &str = "x-zcode-rpc-host-capability";
/// 响应体上限：server-info / capability 都是小对象，超过即视为协议错误。
const BODY_LIMIT: usize = 1024 * 1024;

/// 一次探测的结果：远端身份 + 可用于 `/ws/host` 升级的 capability。
#[derive(Debug)]
pub struct ServerProbe {
    pub info: ServerRemoteInfo,
    pub capability: String,
}

/// 按 server URL 做 HTTP 探测；调用方负责超时与身份判定顺序。
pub async fn probe(url: &str, token: Option<&str>) -> Result<ServerProbe> {
    let endpoints = resolve_server_remote_endpoints(url)?;
    probe_server(&endpoints, token).await
}

/// 用已取得的探测结果升级 WebSocket 并建立帧泵；capability 只能使用一次。
pub async fn open(url: &str, token: Option<&str>, probe: ServerProbe) -> Result<RemoteConnection> {
    let endpoints = resolve_server_remote_endpoints(url)?;
    let socket = open_websocket(&endpoints, token, &probe.capability).await?;
    Ok(bridge(socket, connection_info(&probe.info)))
}

/// `GET /api/server-info` + `POST /api/rpc-host-capability`。
pub async fn probe_server(
    endpoints: &ServerRemoteEndpoints,
    token: Option<&str>,
) -> Result<ServerProbe> {
    let info = fetch_json(&endpoints.info_url, "GET", token).await?;
    let info = parse_server_info(&info)?;
    let capability = fetch_json(&endpoints.host_capability_url, "POST", token).await?;
    let capability = parse_host_capability(&capability)?;
    Ok(ServerProbe { info, capability })
}

/// 升级 `/ws/host`；`wss` 依赖进程级 ring provider（幂等安装）。
pub async fn open_websocket(
    endpoints: &ServerRemoteEndpoints,
    token: Option<&str>,
    capability: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = authenticated_url(&endpoints.host_ws_url, token)?;
    let mut request = url
        .as_str()
        .into_client_request()
        .context("invalid websocket url")?;
    if let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .context("invalid authorization header")?;
        request.headers_mut().insert("authorization", value);
    }
    let capability = HeaderValue::from_str(capability).context("invalid host capability header")?;
    request
        .headers_mut()
        .insert(HOST_CAPABILITY_HEADER, capability);
    let (socket, _response) = tokio_tungstenite::connect_async(request)
        .await
        .context("websocket upgrade failed")?;
    Ok(socket)
}

/// WebSocket 字节 ↔ [`RemoteChannelClient`] 帧泵；关闭时置位 `closed`。
pub fn bridge(
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    info: ConnectionInfo,
) -> RemoteConnection {
    let (outbound, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    let (closed_tx, _) = watch::channel(false);
    let (mut writer, mut reader) = socket.split();

    let writer_closed = closed_tx.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(bytes) = frames.recv().await {
            // 与 TS `wrapNodeWebSocket.write` 一致：一帧一个二进制消息。
            if writer.send(Message::binary(bytes)).await.is_err() {
                writer_closed.send_replace(true);
                return;
            }
        }
    });

    let reader_client = Arc::clone(&client);
    let reader_closed = closed_tx.clone();
    let reader_task = tokio::spawn(async move {
        let mut decoder = FrameDecoder::new();
        loop {
            let chunk = match reader.next().await {
                Some(Ok(Message::Binary(bytes))) => bytes.to_vec(),
                Some(Ok(Message::Text(text))) => text.as_bytes().to_vec(),
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => continue,
                Some(Err(error)) => {
                    logging::warn("source-server", format!("websocket read failed: {error}"));
                    break;
                }
            };
            decoder.push(&chunk);
            if !drain_frames(&mut decoder, &reader_client) {
                reader_closed.send_replace(true);
                return;
            }
        }
        reader_client.dispose();
        reader_closed.send_replace(true);
    });

    RemoteConnection::from_link(client, info, vec![writer_task, reader_task], closed_tx)
}

/// 处理已解出的全部帧；返回 `false` 表示链路必须关闭。
fn drain_frames(decoder: &mut FrameDecoder, client: &RemoteChannelClient) -> bool {
    loop {
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                if client.handle_frame(&frame).is_err() {
                    client.dispose();
                    return false;
                }
            }
            Ok(None) => return true,
            Err(error) => {
                logging::warn("source-server", format!("frame decode failed: {error}"));
                client.dispose();
                return false;
            }
        }
    }
}

/// `/api/server-info` 到连接身份的投影：server 不披露进程身份，保持 `None`。
pub(crate) fn connection_info(info: &ServerRemoteInfo) -> ConnectionInfo {
    ConnectionInfo {
        version: info.version.clone(),
        // 兼容清单只来自 stdio hello；server 目标版本由服务端自身管理，不做本地比对。
        compatibility: None,
        platform: None,
        arch: None,
        pid: None,
    }
}

async fn fetch_json(url: &str, method: &str, token: Option<&str>) -> Result<Value> {
    let authenticated = authenticated_url(url, token)?;
    let client = escode_cli_net::client(escode_cli_net::ClientOptions::new(
        escode_cli_net::Purpose::WebFetch,
        authenticated.clone(),
    ))
    .await
    .context("network client unavailable")?;
    // capture 只记录去掉 token 的 URL：凭据不得进入日志。
    escode_cli_net::capture::record(method, url);
    let mut request = if method == "POST" {
        client.post(&authenticated)
    } else {
        client.get(&authenticated)
    };
    if let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut response = request.send().await.context("server request failed")?;
    let status = response.status();
    if !status.is_success() {
        bail!("server request failed: HTTP {}", status.as_u16());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("server response failed")? {
        if body.len() + chunk.len() > BODY_LIMIT {
            bail!("server response exceeded {BODY_LIMIT} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).context("server response is not JSON")
}

#[cfg(test)]
#[path = "source_server_websocket_probe_tests.rs"]
mod probe_tests;

#[cfg(test)]
#[path = "source_server_websocket_bridge_tests.rs"]
mod bridge_tests;
