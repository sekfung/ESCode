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
use std::{collections::HashMap, sync::Arc};

type Headers = HashMap<reqwest_mcp::header::HeaderName, reqwest_mcp::header::HeaderValue>;
type Stream = BoxStream<'static, Result<Sse, SseError>>;

#[derive(Clone)]
pub(super) struct Capture<C> {
    inner: C,
    raw: Responses,
}

impl<C> Capture<C> {
    pub fn new(inner: C, raw: Responses) -> Self {
        Self { inner, raw }
    }
    fn tee(&self, stream: Stream) -> Stream {
        let raw = self.raw.clone();
        stream
            .inspect(move |event| {
                if let Ok(Sse { data: Some(data), .. }) = event {
                    raw.offer(data.as_bytes());
                }
            })
            .boxed()
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
            StreamableHttpPostResponse::Sse(stream, session) if discover => first_reply(stream, session).await,
            StreamableHttpPostResponse::Sse(stream, session) => Ok(StreamableHttpPostResponse::Sse(self.tee(stream), session)),
            other => Ok(other),
        }
    }
}

fn is_discover(message: &ClientJsonRpcMessage) -> bool {
    matches!(message, ClientJsonRpcMessage::Request(request) if matches!(request.request, ClientRequest::DiscoverRequest(_)))
}

/// rmcp 启动阶段读 SSE 应答（`expect_initialized`）只认 `Response`、跳过 `Error`，legacy server 以 SSE 回的
/// `server/discover` 错误因此被当作空流，连接失败；Node 判为 legacy 后回落 `initialize`
/// （docs/specs/rust-mcp-parity.md「SSE 应答的 server/discover 错误」）。这里读到第一条应答或错误即转为 JSON 应答。
async fn first_reply<E>(mut stream: Stream, session: Option<String>) -> Result<StreamableHttpPostResponse, StreamableHttpError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    while let Some(event) = stream.next().await {
        let payload = event?.data.unwrap_or_default();
        if payload.trim().is_empty() {
            continue;
        }
        let message: ServerJsonRpcMessage = serde_json::from_str(&payload)?;
        if matches!(message, ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)) {
            return Ok(StreamableHttpPostResponse::Json(message, session));
        }
    }
    Err(StreamableHttpError::UnexpectedServerResponse("empty sse stream".into()))
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
        let response = self.inner.post_message(uri, message, session_id, auth, headers).await?;
        self.adapt(discover, response).await
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
        let response =
            self.inner.post_message_with_max_sse_event_size(uri, message, session_id, auth, headers, max).await?;
        self.adapt(discover, response).await
    }
    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<(), StreamableHttpError<C::Error>> {
        self.inner.delete_session(uri, session_id, auth, headers).await
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth: Option<String>,
        headers: Headers,
    ) -> Result<Stream, StreamableHttpError<C::Error>> {
        let stream = self.inner.get_stream(uri, session_id, last_event_id, auth, headers).await?;
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
        let stream =
            self.inner.get_stream_with_max_sse_event_size(uri, session_id, last_event_id, auth, headers, max).await?;
        Ok(self.tee(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(data: &[&str]) -> Stream {
        let items: Vec<Result<Sse, SseError>> =
            data.iter().map(|d| Ok(Sse { data: Some((*d).to_owned()), ..Default::default() })).collect();
        futures_util::stream::iter(items).boxed()
    }

    #[tokio::test]
    async fn discover_error_over_sse_becomes_json_reply() {
        let error = r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"legacy"}}"#;
        let reply = first_reply::<std::io::Error>(events(&["", error]), Some("s1".into())).await.unwrap();
        assert!(matches!(reply, StreamableHttpPostResponse::Json(ServerJsonRpcMessage::Error(_), Some(ref s)) if s == "s1"));
    }

    #[tokio::test]
    async fn stream_without_reply_keeps_rmcp_error() {
        let result = first_reply::<std::io::Error>(events(&[]), None).await;
        assert!(matches!(result, Err(StreamableHttpError::UnexpectedServerResponse(_))));
    }
}
