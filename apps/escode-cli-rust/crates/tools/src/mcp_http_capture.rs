//! Streamable HTTP 应答原文旁路（docs/specs/rust-tool-schema-order.md 第 5 条）：rmcp 把 SSE 事件解析成
//! 排序的 serde_json Map，声明顺序只存在于事件原文。这里包住任意 HTTP client，把 POST 应答与 GET 流中每个
//! SSE 事件的 `data` 在交给 rmcp 之前交给捕获表；另把 `server/discover` 的 SSE 应答转为 JSON 应答（见 `first_reply`）。
//! 其余行为原样委托。
use super::mcp_raw_capture::Responses;
use futures_util::{StreamExt, stream::BoxStream};
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::Sse;
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

type Headers = HashMap<reqwest_mcp::header::HeaderName, reqwest_mcp::header::HeaderValue>;
type Stream = BoxStream<'static, Result<Sse, SseError>>;

/// Auto 探测超时在错误链中的标记。`mcp_connection` 用它在 `failureKind` 上对齐 Node：
/// TS SDK 的 `RequestTimeout` 落到默认 `network_unreachable`（docs/specs/rust-mcp-parity.md「协议协商默认值」）。
pub(super) const PROBE_TIMEOUT_PREFIX: &str = "version negotiation probe timed out after ";

#[derive(Clone)]
pub(super) struct Capture<C> {
    inner: C,
    raw: Responses,
    /// Auto 模式的 `server/discover` 预算；None 表示 pin/legacy，不额外限时。
    probe_budget: Option<Duration>,
}

impl<C> Capture<C> {
    pub fn new(inner: C, raw: Responses, probe_budget: Option<Duration>) -> Self {
        Self {
            inner,
            raw,
            probe_budget,
        }
    }
    fn tee(&self, stream: Stream) -> Stream {
        let raw = self.raw.clone();
        stream
            .inspect(move |event| {
                if let Ok(Sse {
                    data: Some(data), ..
                }) = event
                {
                    raw.offer(data.as_bytes());
                }
            })
            .boxed()
    }
    /// Auto + HTTP 的探测超时在 TS SDK 中是硬失败（`RequestTimeout`），不回落 legacy；
    /// 因此这里返回传输错误而不是伪造 legacy 应答（docs/specs/rust-mcp-parity.md「协议协商默认值」）。
    /// 预算内返回的应答仍走原有解析；超时后 POST 被取消，server 只会看到该请求被中止。
    async fn post<F, E>(
        &self,
        discover: bool,
        send: F,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<E>>
    where
        F: Future<Output = Result<StreamableHttpPostResponse, StreamableHttpError<E>>>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let response = match (discover, self.probe_budget) {
            (true, Some(budget)) => match tokio::time::timeout(budget, send).await {
                Ok(result) => result?,
                Err(_) => {
                    return Err(StreamableHttpError::UnexpectedServerResponse(
                        format!("{PROBE_TIMEOUT_PREFIX}{}ms", budget.as_millis()).into(),
                    ));
                }
            },
            _ => send.await?,
        };
        self.adapt(discover, response).await
    }

    async fn adapt<E>(
        &self,
        discover: bool,
        response: StreamableHttpPostResponse,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match response {
            StreamableHttpPostResponse::Sse(stream, session) if discover => {
                first_reply(stream, session).await
            }
            StreamableHttpPostResponse::Sse(stream, session) => {
                Ok(StreamableHttpPostResponse::Sse(self.tee(stream), session))
            }
            other => Ok(other),
        }
    }
}

/// 只把本模块自己产生的探测超时标记归类为 Node 的 `RequestTimeout` 路径，
/// 不按任意错误文本放宽 `protocol_negotiation_failed` 的判定。
pub(super) fn is_probe_timeout(error: &rmcp::service::ClientInitializeError) -> bool {
    matches!(
        error,
        rmcp::service::ClientInitializeError::TransportError { error, .. }
            if error.to_string().contains(PROBE_TIMEOUT_PREFIX)
    )
}

/// HTTP Auto 探测超时在 Node 侧是 SDK `RequestTimeout`，最终落到默认 `network_unreachable`；
/// 其余错误按 Node 的传输默认值分类，只有协商类错误改写成 `protocol_negotiation_failed`
/// （docs/specs/rust-mcp-parity.md）。
pub(super) fn init_failure(
    error: rmcp::service::ClientInitializeError,
    probing: bool,
) -> anyhow::Error {
    if is_probe_timeout(&error) {
        anyhow::anyhow!("network_unreachable")
    } else {
        super::mcp_init_failure::classify(
            error,
            super::mcp_init_failure::DefaultFailure::NetworkUnreachable,
            probing,
        )
    }
}

fn is_discover(message: &ClientJsonRpcMessage) -> bool {
    matches!(message, ClientJsonRpcMessage::Request(request) if matches!(request.request, ClientRequest::DiscoverRequest(_)))
}

/// rmcp 启动阶段读 SSE 应答（`expect_initialized`）只认 `Response`、跳过 `Error`，legacy server 以 SSE 回的
/// `server/discover` 错误因此被当作空流，连接失败；Node 判为 legacy 后回落 `initialize`
/// （docs/specs/rust-mcp-parity.md「SSE 应答的 server/discover 错误」）。这里读到第一条应答或错误即转为 JSON 应答。
async fn first_reply<E>(
    mut stream: Stream,
    session: Option<String>,
) -> Result<StreamableHttpPostResponse, StreamableHttpError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    while let Some(event) = stream.next().await {
        let payload = event?.data.unwrap_or_default();
        if payload.trim().is_empty() {
            continue;
        }
        let message: ServerJsonRpcMessage = serde_json::from_str(&payload)?;
        if matches!(
            message,
            ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
        ) {
            return Ok(StreamableHttpPostResponse::Json(message, session));
        }
    }
    Err(StreamableHttpError::UnexpectedServerResponse(
        "empty sse stream".into(),
    ))
}

impl<C: StreamableHttpClient + Sync> StreamableHttpClient for Capture<C> {
    type Error = C::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<C::Error>> {
        let discover = is_discover(&message);
        self.post(
            discover,
            self.inner
                .post_message(uri, message, session_id, auth, headers),
        )
        .await
    }
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth: Option<String>,
        headers: Headers,
        max: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<C::Error>> {
        let discover = is_discover(&message);
        self.post(
            discover,
            self.inner
                .post_message_with_max_sse_event_size(uri, message, session_id, auth, headers, max),
        )
        .await
    }
    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<(), StreamableHttpError<C::Error>> {
        self.inner
            .delete_session(uri, session_id, auth, headers)
            .await
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<Stream, StreamableHttpError<C::Error>> {
        let stream = self
            .inner
            .get_stream(uri, session_id, last_event_id, auth, headers)
            .await?;
        Ok(self.tee(stream))
    }
    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth: Option<String>,
        headers: Headers,
        max: usize,
    ) -> Result<Stream, StreamableHttpError<C::Error>> {
        let stream = self
            .inner
            .get_stream_with_max_sse_event_size(uri, session_id, last_event_id, auth, headers, max)
            .await?;
        Ok(self.tee(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(data: &[&str]) -> Stream {
        let items: Vec<Result<Sse, SseError>> = data
            .iter()
            .map(|d| {
                Ok(Sse {
                    data: Some((*d).to_owned()),
                    ..Default::default()
                })
            })
            .collect();
        futures_util::stream::iter(items).boxed()
    }

    #[tokio::test]
    async fn discover_error_over_sse_becomes_json_reply() {
        let error = r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"legacy"}}"#;
        let reply = first_reply::<std::io::Error>(events(&["", error]), Some("s1".into()))
            .await
            .unwrap();
        assert!(
            matches!(reply, StreamableHttpPostResponse::Json(ServerJsonRpcMessage::Error(_), Some(ref s)) if s == "s1")
        );
    }

    #[tokio::test]
    async fn stream_without_reply_keeps_rmcp_error() {
        let result = first_reply::<std::io::Error>(events(&[]), None).await;
        assert!(matches!(
            result,
            Err(StreamableHttpError::UnexpectedServerResponse(_))
        ));
    }

    fn init_transport_error(message: &str) -> rmcp::service::ClientInitializeError {
        rmcp::service::ClientInitializeError::TransportError {
            error: rmcp::transport::DynamicTransportError::from_parts(
                "streamable-http",
                std::any::TypeId::of::<()>(),
                Box::new(std::io::Error::other(message)),
            ),
            context: "discover".into(),
        }
    }

    #[test]
    fn probe_timeout_classifies_as_network_unreachable() {
        let error = init_failure(
            init_transport_error(&format!("{PROBE_TIMEOUT_PREFIX}400ms")),
            true,
        );
        assert_eq!(error.to_string(), "network_unreachable");
    }

    #[test]
    fn other_initialization_errors_keep_protocol_negotiation_failed() {
        let error = init_failure(init_transport_error("connection refused"), true);
        assert_eq!(error.to_string(), "protocol_negotiation_failed");
    }
}
