//! 官方 MCP（zcode_official）的 Streamable HTTP client（docs/specs/rust-mcp-official-auth.md 第 2 期），
//! 对齐 TS `createOfficialMcpAuthFetch`：逐请求校验 origin（不受信任时 fail closed、零网络请求）、经 Host 通道
//! 取身份头（不缓存，取不到则匿名降级）、合并时覆盖同名并剔除保留头、不跟随重定向、已注入身份的 401 重试一次，
//! 401/403/3xx 分类失败；非 tools/call 响应按状态与有界 JSON 体做连接诊断。成功响应的解析与 rmcp reqwest 实现一致。
use super::mcp_config::Server;
use crate::contract::{Event, EventSink};
use crate::domain::mcp_official_auth as rules;
use futures_util::{StreamExt, stream::BoxStream};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use serde_json::{Value, json};
use sse_stream::{Sse, SseStream};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

type Headers = HashMap<reqwest_mcp::header::HeaderName, reqwest_mcp::header::HeaderValue>;
type HttpError = StreamableHttpError<reqwest_mcp::Error>;
const EVENT_STREAM: &str = "text/event-stream";
const JSON_MIME: &str = "application/json";
const SESSION_HEADER: &str = "Mcp-Session-Id";
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
/// TS 身份头端口的进程级请求序号（`official-mcp-auth:<n>`）。
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 连接期的官方鉴权事实：最后一次鉴权失败分类与最后一次连接诊断（failureKind, 服务端 request id）。
pub(super) struct Official {
    pub name: String,
    origin: Option<String>,
    plugin_id: String,
    mcp_key: String,
    workspace: Value,
    host: Option<EventSink>,
    dev_trusted: Option<String>,
    api_origin: Option<String>,
    pub failure: Mutex<Option<&'static str>>,
    pub diagnostic: Mutex<Option<(&'static str, Option<String>)>>,
}
impl Official {
    /// 仅对插件加载器写入 provenance 的 http server 生效；workspace 引用按仓库约定 `identity || path`。
    pub fn from_server(server: &Server, cwd: &Path, host: Option<EventSink>) -> Option<Arc<Self>> {
        let official = server.raw.get("official")?;
        let env = |key: &str| std::env::var(key).ok();
        let api_origin =
            rules::zcode_api_origin(env("ZCODE_BASE_URL").as_deref(), env("ZCODE_ENDPOINT_ORIGIN").as_deref()).ok();
        // http 以配置 endpoint 为目标；stdio 没有 url，目标 origin 由宿主给出（ZCode API origin）。
        let origin = match server.transport.as_str() {
            "http" => origin_of(server.raw["url"].as_str()?),
            "stdio" => api_origin.clone(),
            _ => return None,
        };
        let path = cwd.display().to_string();
        let identity = std::env::var("ZCODE_WORKSPACE_IDENTITY").ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let mut workspace = json!({"workspaceKey": identity.clone().unwrap_or_else(|| path.clone()), "workspacePath": path});
        if let Some(identity) = identity {
            workspace["workspaceIdentity"] = identity.into();
        }
        Some(Arc::new(Self {
            name: server.name.clone(),
            origin,
            plugin_id: official["pluginId"].as_str()?.into(),
            mcp_key: official["mcpKey"].as_str()?.into(),
            workspace,
            host,
            dev_trusted: env(rules::DEV_TRUSTED_ORIGINS_ENV),
            api_origin,
            failure: Mutex::new(None),
            diagnostic: Mutex::new(None),
        }))
    }
    /// 请求 origin 必须等于配置 endpoint 的 origin，且通过信任判定。
    fn trusted(&self, url: &str) -> Option<String> {
        let origin = origin_of(url)?;
        (Some(&origin) == self.origin.as_ref()
            && rules::origin_trusted(&origin, self.dev_trusted.as_deref(), self.api_origin.as_deref()).0)
            .then_some(origin)
    }
    /// TS `createOfficialMcpAuthHeadersPort`：每次请求向 Host 取身份头；失败只记 reason，不带出值。
    async fn identity(&self, origin: &str) -> Vec<(String, String)> {
        match self.resolve(origin).await {
            Ok(headers) => headers.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect(),
            Err(reason) => {
                eprintln!("zcode-cli-rust: official MCP {} auth headers unavailable: {reason}", self.name);
                vec![]
            }
        }
    }
    /// stdio 出站消息的 `_meta` 身份载荷（TS `resolveOfficialStdioAuthMeta`）：失败也下发枚举 reason。
    pub async fn stdio_meta(&self) -> Value {
        let fail = |reason: &str| {
            eprintln!("zcode-cli-rust: official MCP {} stdio auth headers unavailable: {reason}", self.name);
            json!({"ok": false, "reason": reason})
        };
        if self.host.is_none() {
            return fail("official_auth_unavailable");
        }
        let Some(target) = self.origin.clone() else {
            return fail("official_auth_unavailable");
        };
        if !rules::origin_trusted(&target, self.dev_trusted.as_deref(), self.api_origin.as_deref()).0 {
            return fail("official_mcp_origin_untrusted");
        }
        match self.resolve(&target).await {
            Ok(headers) => json!({"ok": true, "headers": headers}),
            Err(reason) => fail(&reason),
        }
    }
    /// TS `createOfficialMcpAuthHeadersPort`：向 Host 取身份头；Host 不可达按 `official_auth_unavailable`。
    async fn resolve(&self, origin: &str) -> Result<serde_json::Map<String, Value>, String> {
        let unavailable = || "official_auth_unavailable".to_owned();
        let Some(host) = &self.host else {
            return Err(unavailable());
        };
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
        let params = json!({
            "requestId": format!("official-mcp-auth:{sequence}"),
            "workspace": self.workspace,
            "pluginId": self.plugin_id,
            "mcpKey": self.mcp_key,
            "targetOrigin": origin,
        });
        let (reply, receive) = tokio::sync::oneshot::channel();
        let method = "interaction/requestOfficialMcpAuthHeaders".to_owned();
        if host.send(Event::HostRequest { method, params, reply }).await.is_err() {
            return Err(unavailable());
        }
        let reply = match receive.await {
            Ok(Ok(text)) => serde_json::from_str::<Value>(&text).unwrap_or_default(),
            _ => Value::Null,
        };
        match reply["headers"].as_object().filter(|_| reply["ok"] == true) {
            Some(headers) => Ok(headers.clone()),
            None => Err(reply["reason"].as_str().map(str::to_owned).unwrap_or_else(unavailable)),
        }
    }
}

fn origin_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok().map(|u| u.origin().ascii_serialization())
}

/// 已发送请求的结果：状态、响应头，以及已读完的响应体（JSON/失败）或未读的流（SSE）。
struct Fetched {
    status: reqwest_mcp::StatusCode,
    content_type: Option<String>,
    session_id: Option<String>,
    body: Result<Vec<u8>, reqwest_mcp::Response>,
}

#[derive(Clone)]
pub(super) struct OfficialClient {
    http: reqwest_mcp::Client,
    pub official: Arc<Official>,
}
impl OfficialClient {
    pub fn new(http: reqwest_mcp::Client, official: Arc<Official>) -> Self {
        Self { http, official }
    }
    fn fail(&self, kind: &'static str, message: &str) -> HttpError {
        *self.official.failure.lock().unwrap() = Some(kind);
        StreamableHttpError::UnexpectedServerResponse(message.to_owned().into())
    }
    async fn official_fetch(
        &self,
        url: &str,
        rpc_method: Option<String>,
        incoming: Vec<(String, String)>,
        build: impl Fn() -> reqwest_mcp::RequestBuilder,
    ) -> Result<Fetched, HttpError> {
        let Some(origin) = self.official.trusted(url) else {
            let message = format!("official MCP origin is not trusted: {}", self.official.name);
            return Err(self.fail("official_mcp_origin_untrusted", &message));
        };
        let mut attempt = 0;
        let response = loop {
            attempt += 1;
            let identity = self.official.identity(&origin).await;
            let mut request = build();
            for (name, value) in rules::merge_headers(&incoming, &identity) {
                request = request.header(name, value);
            }
            let response = request.send().await?;
            // 只有实际注入过凭证的请求才在 401 后重试一次（窄竞态：请求发出后凭证恰好被刷新）。
            if response.status() == reqwest_mcp::StatusCode::UNAUTHORIZED && !identity.is_empty() && attempt == 1 {
                continue;
            }
            break response;
        };
        let status = response.status();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let content_type = header("content-type");
        let session_id = header(SESSION_HEADER);
        let request_id = header("x-request-id");
        let streaming = status.is_success() && content_type.as_deref().is_some_and(|ct| ct.starts_with(EVENT_STREAM));
        let body = if streaming { Err(response) } else { Ok(response.bytes().await?.to_vec()) };
        if rpc_method.as_deref() != Some("tools/call") {
            let text = body.as_ref().ok().filter(|b| b.len() <= MAX_DIAGNOSTIC_BYTES).map(|b| String::from_utf8_lossy(b).into_owned());
            if let Some(kind) = rules::classify_response(status.as_u16(), content_type.as_deref().unwrap_or_default(), text.as_deref()) {
                *self.official.diagnostic.lock().unwrap() = Some((kind, request_id.clone()));
            }
        }
        if let Some(kind) = rules::auth_failure(status.as_u16()) {
            let message = match kind {
                "official_auth_rejected" => "official MCP rejected the current credential".to_owned(),
                "official_auth_forbidden" => "official MCP denied access for the current plan".to_owned(),
                _ => format!("official MCP responded with a blocked redirect ({})", status.as_u16()),
            };
            return Err(self.fail(kind, &message));
        }
        Ok(Fetched { status, content_type, session_id, body })
    }
}

fn pairs(headers: &Headers, session: Option<&str>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
        .collect();
    if let Some(session) = session {
        pairs.push((SESSION_HEADER.into(), session.into()));
    }
    pairs
}

/// rmcp `bounded_sse_stream` 的等价物（该函数为 crate 私有）：单个事件超过上限即报错。
fn bounded_sse(response: reqwest_mcp::Response, max: usize) -> BoxStream<'static, Result<Sse, SseError>> {
    let (mut size, mut newline) = (0usize, false);
    let bytes = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        for &byte in chunk.iter() {
            match byte {
                b'\r' => {}
                b'\n' if newline => size = 0,
                b'\n' => newline = true,
                _ => {
                    newline = false;
                    size += 1;
                    if size > max {
                        return Err(std::io::Error::other("MCP SSE event exceeds size limit"));
                    }
                }
            }
        }
        Ok(chunk)
    });
    SseStream::from_bytes_stream(bytes).boxed()
}

fn is_reply_free(message: &ClientJsonRpcMessage) -> bool {
    matches!(
        message,
        ClientJsonRpcMessage::Notification(_) | ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_)
    )
}

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
        self.post_message_with_max_sse_event_size(uri, message, session_id, None, headers, 8 * 1024 * 1024)
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
        let method = serde_json::to_value(&message).ok().and_then(|v| v["method"].as_str().map(str::to_owned));
        let incoming = pairs(&headers, session_id.as_deref());
        let build = || {
            self.http
                .post(uri.as_ref())
                .header("Accept", format!("{EVENT_STREAM}, {JSON_MIME}"))
                .json(&message)
        };
        let fetched = self.official_fetch(&uri, method, incoming, build).await?;
        let status = fetched.status;
        if matches!(status, reqwest_mcp::StatusCode::ACCEPTED | reqwest_mcp::StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest_mcp::StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        let json = fetched.content_type.as_deref().is_some_and(|ct| ct.starts_with(JSON_MIME));
        let body = match fetched.body {
            Err(response) => return Ok(StreamableHttpPostResponse::Sse(bounded_sse(response, max), fetched.session_id)),
            Ok(body) => body,
        };
        if status.is_success() && body.is_empty() && is_reply_free(&message) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            if json && let Ok(error @ JsonRpcMessage::Error(_)) = serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                return Ok(StreamableHttpPostResponse::Json(error, fetched.session_id));
            }
            let text = String::from_utf8_lossy(&body);
            return Err(StreamableHttpError::UnexpectedServerResponse(format!("HTTP {status}: {text}").into()));
        }
        if !json {
            return Err(StreamableHttpError::UnexpectedContentType(fetched.content_type));
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
        self.get_stream_with_max_sse_event_size(uri, session_id, last_event_id, None, headers, 8 * 1024 * 1024)
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
        let build = || self.http.get(uri.as_ref()).header("Accept", format!("{EVENT_STREAM}, {JSON_MIME}"));
        let fetched = self.official_fetch(&uri, None, incoming, build).await?;
        if fetched.status == reqwest_mcp::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        match fetched.body {
            Err(response) => Ok(bounded_sse(response, max)),
            Ok(_) if !fetched.status.is_success() => Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {}", fetched.status).into(),
            )),
            Ok(_) => Err(StreamableHttpError::UnexpectedContentType(fetched.content_type)),
        }
    }
}

/// 连接失败的官方分类（TS `failConnection`）：不受信任 origin 优先，其次连接期响应诊断（带服务端 request id）。
#[derive(Debug)]
pub(super) struct OfficialFailure {
    pub kind: &'static str,
    pub server_request_id: Option<String>,
}
impl std::fmt::Display for OfficialFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "official MCP connection failed ({})", self.kind)
    }
}
impl std::error::Error for OfficialFailure {}
impl Official {
    pub fn connect_failure(&self) -> Option<OfficialFailure> {
        if *self.failure.lock().unwrap() == Some("official_mcp_origin_untrusted") {
            return Some(OfficialFailure { kind: "official_origin_untrusted", server_request_id: None });
        }
        let (kind, server_request_id) = self.diagnostic.lock().unwrap().clone()?;
        Some(OfficialFailure { kind, server_request_id })
    }
}
