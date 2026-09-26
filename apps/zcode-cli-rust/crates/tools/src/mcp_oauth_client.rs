//! 带 OAuth 的 Streamable HTTP client（docs/specs/rust-mcp-oauth.md 第 2 层，对齐 TS 运行期 `AuthProvider`）：
//! 每个请求前取共享凭据中的 token（临期先刷新），401 在跨进程锁内刷新后重试一次。
//! 需要交互授权或刷新失败时原样返回 401/403，并把分类结果记在 `last`，由连接编排读取。
use super::mcp_oauth_credentials::Auth;
use super::mcp_oauth_flow::AuthorizationRequired;
use crate::domain::mcp_oauth as rules;
use futures_util::stream::BoxStream;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::Sse;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

type Headers = HashMap<reqwest_mcp::header::HeaderName, reqwest_mcp::header::HeaderValue>;
type HttpError = StreamableHttpError<reqwest_mcp::Error>;

/// 连接失败时的 OAuth 分类（TS `classifyInteractiveAuthorizationTrigger`）。
pub(super) enum Failure {
    /// 需要交互授权：无凭据/无 refresh token/invalid_grant、重试后仍 401、403 insufficient_scope。
    Required(AuthorizationRequired),
    /// 临时刷新失败或配置错误：不触发交互授权（原因写入日志）。
    Other,
}

#[derive(Clone)]
pub(super) struct AuthClient {
    inner: reqwest_mcp::Client,
    pub oauth: Auth,
    pub last: Arc<Mutex<Option<Failure>>>,
}
impl AuthClient {
    pub fn new(inner: reqwest_mcp::Client, oauth: Auth) -> Self {
        Self {
            inner,
            oauth,
            last: Default::default(),
        }
    }
    pub fn take_failure(&self) -> Option<Failure> {
        self.last.lock().unwrap().take()
    }
    fn record(&self, failure: Failure) {
        *self.last.lock().unwrap() = Some(failure);
    }
    fn required(&self, reason: &str) {
        self.record(Failure::Required(AuthorizationRequired {
            reason: reason.into(),
            ..Default::default()
        }));
    }
    async fn token(&self) -> Result<Option<String>, HttpError> {
        self.oauth.token().await.map_err(|error| {
            self.classify(error);
            StreamableHttpError::UnexpectedServerResponse("MCP OAuth token unavailable".into())
        })
    }
    fn classify(&self, error: anyhow::Error) {
        self.record(match error.downcast::<AuthorizationRequired>() {
            Ok(required) => Failure::Required(required),
            Err(error) => {
                eprintln!("zcode-cli-rust: MCP server {} OAuth token unavailable: {error:#}", self.oauth.name());
                Failure::Other
            }
        });
    }
    /// SDK 运行期 AuthProvider：401 → `onUnauthorized()` → 以新 token 重试一次，仍 401 为 `unauthorized`；
    /// 403 insufficient_scope 不刷新，直接交给编排层做 step-up（scope 与 resource_metadata 取自 challenge）。
    async fn authorized<T, F, Fut>(&self, op: F) -> Result<T, HttpError>
    where
        F: Fn(Option<String>) -> Fut,
        Fut: Future<Output = Result<T, HttpError>>,
    {
        if matches!(self.oauth, Auth::Plain) {
            return op(None).await;
        }
        let result = op(self.token().await?).await;
        let challenge = match &result {
            Err(StreamableHttpError::AuthRequired(e)) => e.www_authenticate_header.clone(),
            // client_credentials 按 SDK：403 以 challenge scope 重新取 token 后重试一次，没有交互授权。
            Err(StreamableHttpError::InsufficientScope(e)) if matches!(self.oauth, Auth::Credentials(_)) => {
                e.www_authenticate_header.clone()
            }
            Err(StreamableHttpError::InsufficientScope(e)) => {
                let challenge = rules::parse_challenge(&e.www_authenticate_header);
                self.record(Failure::Required(AuthorizationRequired {
                    reason: "insufficient_scope".into(),
                    required_scope: e.required_scope.clone().or(challenge.scope),
                    resource_metadata_url: challenge.resource_metadata_url,
                }));
                return result;
            }
            _ => return result,
        };
        if let Err(error) = self.oauth.on_unauthorized(&challenge).await {
            self.classify(error);
            return result;
        }
        let retried = op(self.token().await?).await;
        if let Err(StreamableHttpError::AuthRequired(_)) = &retried {
            self.required("unauthorized");
        }
        retried
    }
    /// 旧版 SSE 的 GET/POST 同样走 401 → 刷新 → 重试一次。
    pub async fn send(
        &self,
        build: impl Fn() -> reqwest_mcp::RequestBuilder,
    ) -> anyhow::Result<reqwest_mcp::Response> {
        let with_token = |token: Option<String>| match token {
            Some(token) => build().bearer_auth(token),
            None => build(),
        };
        if matches!(self.oauth, Auth::Plain) {
            return Ok(build().send().await?);
        }
        let first = match self.oauth.token().await {
            Ok(token) => with_token(token).send().await?,
            Err(error) => {
                let message = error.to_string();
                self.classify(error);
                anyhow::bail!(message)
            }
        };
        if first.status() != reqwest_mcp::StatusCode::UNAUTHORIZED {
            return Ok(first);
        }
        let challenge = first
            .headers()
            .get(reqwest_mcp::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if let Err(error) = self.oauth.on_unauthorized(&challenge).await {
            self.classify(error);
            return Ok(first);
        }
        let retried = with_token(self.oauth.token().await?).send().await?;
        if retried.status() == reqwest_mcp::StatusCode::UNAUTHORIZED {
            self.required("unauthorized");
        }
        Ok(retried)
    }
}

impl StreamableHttpClient for AuthClient {
    type Error = reqwest_mcp::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth: Option<String>,
        headers: Headers,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.post_message_with_max_sse_event_size(uri, message, session_id, None, headers, usize::MAX)
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
        self.authorized(|token| {
            self.inner.post_message_with_max_sse_event_size(
                uri.clone(),
                message.clone(),
                session_id.clone(),
                token,
                headers.clone(),
                max,
            )
        })
        .await
    }
    /// Node（SDK 2.0 `transport.close`）关闭连接时不终止会话、不发 DELETE；rmcp 默认会发。
    /// 对齐 Node：官方 MCP 差分中发现该差异（docs/specs/rust-mcp-parity.md「关闭语义」）。
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
        self.get_stream_with_max_sse_event_size(uri, session_id, last_event_id, None, headers, usize::MAX)
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
        self.authorized(|token| {
            self.inner.get_stream_with_max_sse_event_size(
                uri.clone(),
                session_id.clone(),
                last_event_id.clone(),
                token,
                headers.clone(),
                max,
            )
        })
        .await
    }
}
