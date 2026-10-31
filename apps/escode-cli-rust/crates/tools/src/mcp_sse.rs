use super::mcp_config::Server;
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use rmcp::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::sync::{CancellationToken, DropGuard};

type Auth = Option<super::mcp_oauth_client::AuthClient>;
/// eventsource 的缺省重连间隔（docs/specs/rust-mcp-parity.md「legacy SSE 自动重连」）。
const DEFAULT_RETRY: Duration = Duration::from_secs(3);
const MAX_RETRY_MILLIS: u64 = 300_000;

pub(super) struct Transport {
    client: reqwest_mcp::Client,
    auth: Auth,
    endpoint: Arc<RwLock<Arc<str>>>,
    headers: reqwest_mcp::header::HeaderMap,
    rx: mpsc::Receiver<ServerJsonRpcMessage>,
    worker: Option<tokio::task::JoinHandle<()>>,
    cancel: CancellationToken,
    _guard: DropGuard,
}
impl Transport {
    pub async fn open(
        config: &Server,
        client: reqwest_mcp::Client,
        auth: Auth,
        stop: &CancellationToken,
        raw: super::mcp_raw_capture::Responses,
    ) -> Result<Self> {
        let url = url::Url::parse(config.raw["url"].as_str().unwrap())?;
        let mut headers = reqwest_mcp::header::HeaderMap::new();
        if let Some(values) = config.raw["headers"].as_object() {
            for (k, v) in values {
                headers.insert(
                    k.parse::<reqwest_mcp::header::HeaderName>()?,
                    v.as_str().unwrap().parse()?,
                );
            }
        }
        let response = tokio::select! {biased;_=stop.cancelled()=>anyhow::bail!("Cancelled"),response=send(&auth,||client.get(url.clone()).headers(headers.clone()).header("Accept","text/event-stream"))=>response.context("network_unreachable")?};
        let response = response.error_for_status().context("network_unreachable")?;
        ensure!(
            response.status() != reqwest_mcp::StatusCode::NO_CONTENT,
            "SSE server sent 204"
        );
        ensure!(
            is_event_stream(&response),
            "SSE response is not text/event-stream"
        );

        let endpoint_shared = Arc::new(RwLock::new(Arc::<str>::from("")));
        let worker_endpoint = endpoint_shared.clone();
        let (tx, rx) = mpsc::channel(32);
        let (endpoint_tx, ready) = oneshot::channel();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let guard = cancel.clone().drop_guard();
        let worker_client = client.clone();
        let worker_auth = auth.clone();
        let worker_headers = headers.clone();
        let worker =
            escode_cli_core_api::scope::Spawner::current().spawn("mcp-sse-worker", async move {
                let mut endpoint_tx = Some(endpoint_tx);
                let mut last_event_id: Option<String> = None;
                let mut retry = DEFAULT_RETRY;
                let mut pending = Some(response);
                loop {
                    let response = match pending.take() {
                        Some(response) => response,
                        None => {
                            let last_event_id_header = last_event_id
                                .as_ref()
                                .and_then(|id| reqwest_mcp::header::HeaderValue::from_str(id).ok());
                            let build = || {
                                let mut request = worker_client
                                    .get(url.clone())
                                    .headers(worker_headers.clone())
                                    .header("Accept", "text/event-stream");
                                if let Some(value) = last_event_id_header.clone() {
                                    request = request.header(
                                        reqwest_mcp::header::HeaderName::from_static(
                                            "last-event-id",
                                        ),
                                        value,
                                    );
                                }
                                request
                            };
                            match tokio::select! {biased;_=worker_cancel.cancelled()=>return,result=send(&worker_auth,build)=>result} {
                                Ok(response) => {
                                    let Ok(response) = response.error_for_status() else {
                                        return;
                                    };
                                    if response.status() == reqwest_mcp::StatusCode::NO_CONTENT
                                        || !is_event_stream(&response)
                                    {
                                        return;
                                    }
                                    response
                                }
                                Err(_) => {
                                    if !retry_after(retry, &worker_cancel).await {
                                        return;
                                    }
                                    continue;
                                }
                            }
                        }
                    };

                    let mut stream = response.bytes_stream();
                    let mut decoder = Decoder::default();
                    loop {
                        let chunk = tokio::select! {biased;_=worker_cancel.cancelled()=>return,chunk=stream.next()=>chunk};
                        let Some(chunk) = chunk else { break };
                        let Ok(chunk) = chunk else { break };
                        let Ok(events) = decoder.push(&chunk) else {
                            break;
                        };
                        if let Some(millis) = decoder.take_retry() {
                            retry = Duration::from_millis(millis.clamp(1, MAX_RETRY_MILLIS));
                        }
                        for event in events {
                            // eventsource 对每个事件都更新 lastEventId，不区分 event 类型。
                            if let Some(id) = event.id {
                                last_event_id = if id.is_empty() { None } else { Some(id) };
                            }
                            if event.kind == "endpoint" {
                                let parsed = url.join(&event.data);
                                let parsed = match parsed {
                                    Ok(parsed) if parsed.origin() == url.origin() => parsed,
                                    _ => {
                                        if let Some(endpoint_tx) = endpoint_tx.take() {
                                            let _ = endpoint_tx
                                                .send(Err(anyhow::anyhow!("SSE endpoint changed origin")));
                                        }
                                        return;
                                    }
                                };
                                *worker_endpoint.write().await = Arc::from(parsed.as_str());
                                if let Some(endpoint_tx) = endpoint_tx.take() {
                                    let _ = endpoint_tx.send(Ok(parsed.to_string()));
                                }
                            } else if event.kind == "message" || event.kind.is_empty() {
                                // tools/list 原文保留 inputSchema 声明顺序（docs/specs/rust-tool-schema-order.md）。
                                raw.offer(event.data.as_bytes());
                                if let Ok(message) =
                                    serde_json::from_str::<ServerJsonRpcMessage>(&event.data)
                                    && tx.send(message).await.is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                    // GET 流结束/网络错误：与 eventsource 一样按 retry 间隔重连，不重试已发出的 POST。
                    if !retry_after(retry, &worker_cancel).await {
                        return;
                    }
                }
            });
        tokio::select! {biased;_=stop.cancelled()=>anyhow::bail!("Cancelled"), result=ready=>{result.context("protocol_negotiation_failed")??;}};
        Ok(Self {
            client,
            auth,
            endpoint: endpoint_shared,
            headers,
            rx,
            worker: Some(worker),
            cancel,
            _guard: guard,
        })
    }
}
impl rmcp::transport::Transport<RoleClient> for Transport {
    type Error = std::io::Error;
    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl std::future::Future<Output = std::io::Result<()>> + Send + 'static {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let headers = self.headers.clone();
        let cancel = self.cancel.clone();
        let auth = self.auth.clone();
        async move {
            let endpoint = endpoint.read().await.clone();
            let post = || {
                escode_cli_net::capture::record("POST", endpoint.as_ref());
                client
                    .post(endpoint.as_ref())
                    .headers(headers.clone())
                    .json(&item)
            };
            tokio::select! {biased;_=cancel.cancelled()=>Err(std::io::Error::other("MCP SSE closed")),result=send(&auth,post)=>{
                result.and_then(|r|Ok(r.error_for_status()?)).map(|_|()).map_err(|_|std::io::Error::other("MCP SSE post failed"))
            }}
        }
    }
    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        self.rx.recv().await
    }
    async fn close(&mut self) -> std::io::Result<()> {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}
/// 有 OAuth 时带 Bearer 并按 401 → 刷新 → 重试一次发送；否则直接发送。
async fn send(
    auth: &Auth,
    build: impl Fn() -> reqwest_mcp::RequestBuilder,
) -> Result<reqwest_mcp::Response> {
    match auth {
        Some(auth) => auth.send(build).await,
        None => Ok(build().send().await?),
    }
}

fn is_event_stream(response: &reqwest_mcp::Response) -> bool {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

/// 可取消的退避等待；返回 false 表示已取消。
async fn retry_after(delay: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {biased;_=cancel.cancelled()=>false,_=tokio::time::sleep(delay)=>true}
}

#[derive(Default)]
struct Decoder {
    line: Vec<u8>,
    kind: String,
    data: String,
    id: Option<String>,
    retry: Option<u64>,
}
struct Event {
    kind: String,
    data: String,
    id: Option<String>,
}
impl Decoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Event>> {
        let mut events = vec![];
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            ensure!(
                self.line.len() + part.len() <= 8 * 1024 * 1024,
                "MCP SSE line too large"
            );
            self.line.extend_from_slice(part);
            if part.last() != Some(&b'\n') {
                continue;
            }
            let line = std::str::from_utf8(&self.line)?.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(Event {
                        kind: std::mem::take(&mut self.kind),
                        data: std::mem::take(&mut self.data),
                        id: self.id.take(),
                    });
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            } else if let Some(kind) = line.strip_prefix("event:") {
                self.kind = kind.trim().into();
            } else if let Some(id) = line.strip_prefix("id:") {
                self.id = Some(id.strip_prefix(' ').unwrap_or(id).into());
            } else if let Some(retry) = line.strip_prefix("retry:")
                && let Ok(millis) = retry.trim().parse::<u64>()
            {
                self.retry = Some(millis);
            }
            ensure!(
                self.data.len() <= 8 * 1024 * 1024,
                "MCP SSE event too large"
            );
            self.line.clear();
        }
        Ok(events)
    }
    fn take_retry(&mut self) -> Option<u64> {
        self.retry.take()
    }
}

#[cfg(test)]
#[path = "mcp_sse_tests.rs"]
mod tests;
