//! 官方 MCP 的 Streamable HTTP 协议实现（从 mcp_official_client.rs 拆出）。

use super::mcp_official_client::{
    EVENT_STREAM, Headers, HttpError, JSON_MIME, OfficialClient, bounded_sse, is_reply_free, pairs,
};
use futures_util::stream::BoxStream;
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::Sse;
use std::sync::Arc;

impl StreamableHttpClient for OfficialClient {
    type Error = reqwest_mcp::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth: Option<String>,
        headers: Headers,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            None,
            headers,
            8 * 1024 * 1024,
        )
        .await
    }
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth: Option<String>,
        headers: Headers,
        max: usize,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let method = serde_json::to_value(&message)
            .ok()
            .and_then(|v| v["method"].as_str().map(str::to_owned));
        let incoming = pairs(&headers, session_id.as_deref());
        let build = || {
            self.http
                .post(uri.as_ref())
                .header("Accept", format!("{EVENT_STREAM}, {JSON_MIME}"))
                .json(&message)
        };
        let fetched = self.official_fetch(&uri, method, incoming, build).await?;
        let status = fetched.status;
        if matches!(
            status,
            reqwest_mcp::StatusCode::ACCEPTED | reqwest_mcp::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest_mcp::StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        let json = fetched
            .content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with(JSON_MIME));
        let body = match fetched.body {
            Err(response) => {
                return Ok(StreamableHttpPostResponse::Sse(
                    bounded_sse(response, max),
                    fetched.session_id,
                ));
            }
            Ok(body) => body,
        };
        if status.is_success() && body.is_empty() && is_reply_free(&message) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            if json
                && let Ok(error @ JsonRpcMessage::Error(_)) =
                    serde_json::from_slice::<ServerJsonRpcMessage>(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(error, fetched.session_id));
            }
            let text = String::from_utf8_lossy(&body);
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {status}: {text}").into(),
            ));
        }
        if !json {
            return Err(StreamableHttpError::UnexpectedContentType(
                fetched.content_type,
            ));
        }
        match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
            Ok(parsed) => Ok(StreamableHttpPostResponse::Json(parsed, fetched.session_id)),
            Err(_) if is_reply_free(&message) => Ok(StreamableHttpPostResponse::Accepted),
            Err(error) => Err(StreamableHttpError::UnexpectedServerResponse(
                format!("could not parse JSON response as ServerJsonRpcMessage: {error}").into(),
            )),
        }
    }
    /// Node（SDK 2.0 transport.close）关闭时不发 DELETE；对齐后也避免在退出阶段向 Host 取身份头。
    async fn delete_session(
        &self,
        _uri: Arc<str>,
        _session_id: Arc<str>,
        _auth: Option<String>,
        _headers: Headers,
    ) -> Result<(), HttpError> {
        Ok(())
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth: Option<String>,
        headers: Headers,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, HttpError> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            None,
            headers,
            8 * 1024 * 1024,
        )
        .await
    }
    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth: Option<String>,
        headers: Headers,
        max: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, HttpError> {
        let mut incoming = pairs(&headers, session_id.as_deref());
        if let Some(id) = last_event_id {
            incoming.push(("Last-Event-Id".into(), id));
        }
        let build = || {
            self.http
                .get(uri.as_ref())
                .header("Accept", format!("{EVENT_STREAM}, {JSON_MIME}"))
        };
        let fetched = self.official_fetch(&uri, None, incoming, build).await?;
        if fetched.status == reqwest_mcp::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        match fetched.body {
            Err(response) => Ok(bounded_sse(response, max)),
            Ok(_) if !fetched.status.is_success() => {
                Err(StreamableHttpError::UnexpectedServerResponse(
                    format!("HTTP {}", fetched.status).into(),
                ))
            }
            Ok(_) => Err(StreamableHttpError::UnexpectedContentType(
                fetched.content_type,
            )),
        }
    }
}
