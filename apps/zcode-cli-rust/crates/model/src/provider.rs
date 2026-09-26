use super::{
    config::ModelConfig,
    model_failure,
    model_policy::RetryPolicy,
    model_protocol::{self, ApiType, ProtocolStream},
    model_stream::TextBuffer,
    sse::SseDecoder,
};
use crate::contract::{Event, EventSink, ModelFailure, ModelOutput, ModelPort, RetryState};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ModelFailure>;
pub struct HttpModel {
    config: ModelConfig,
    client: std::sync::Arc<tokio::sync::OnceCell<reqwest::Client>>,
    retry: RetryPolicy,
    url: String,
}
impl HttpModel {
    pub fn new(config: ModelConfig) -> Self {
        let retry = RetryPolicy::resolve(&config.retry);
        let url = config.api_type.url(&config.base_url);
        Self {
            config,
            url,
            client: Default::default(),
            retry,
        }
    }
    pub(super) fn with_pool(
        config: ModelConfig,
        client: std::sync::Arc<tokio::sync::OnceCell<reqwest::Client>>,
    ) -> Self {
        Self {
            client,
            ..Self::new(config)
        }
    }
    async fn client(&self) -> Result<&reqwest::Client> {
        self.client
            .get_or_try_init(|| async {
                // 系统证书读取含阻塞 IO；首次请求按需初始化，不能阻塞 stdio actor 的启动与取消。
                let target = self.url.clone();
                tokio::task::spawn_blocking(move || {
                    let mut builder = reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .pool_idle_timeout(Duration::from_secs(90))
                        .tcp_nodelay(true);
                    // 代理解析与 TS `resolveWebFetchProxyForRequest` 一致（显式配置、ZCODE_HTTP_PROXY、
                    // ZCODE_NO_PROXY 与捕获的宿主代理），不依赖 reqwest 默认读取的 HTTP(S)_PROXY。
                    // 见 docs/specs/rust-net-proxy.md；此处只处理环境变量来源，Host 下发配置待接入。
                    let resolution =
                        zcode_cli_domain::net_proxy::resolve_webfetch_proxy_for_request(
                            &target,
                            &zcode_cli_domain::net_proxy::ProxyOptions {
                                http_proxy: None,
                                no_proxy: None,
                                env: std::env::vars().collect(),
                            },
                        );
                    if let Some(proxy) = resolution.proxy_url {
                        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
                    } else if resolution.no_proxy_matched {
                        builder = builder.no_proxy();
                    }
                    builder.build()
                })
                .await
                .map_err(|_| ModelFailure::new("invalid_request", false))?
                .map_err(|e| model_failure::network(&e))
            })
            .await
    }
    async fn request(
        &self,
        body: Bytes,
        attempt: u32,
        output: &mut TextBuffer<'_>,
        auth: &Value,
        native_search: bool,
    ) -> Result<ModelOutput> {
        let idle_ms = if self.config.stream_idle_timeout_ms == 0 {
            0
        } else {
            self.config
                .stream_idle_timeout_ms
                .saturating_add(u64::from(attempt - 1) * 30_000)
        };
        let mut request = self
            .client()
            .await?
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(body);
        if self.config.api_type == ApiType::Anthropic {
            request = request.header("anthropic-version", "2023-06-01");
        }
        if let Some(seconds) = self.config.request_timeout_seconds {
            request = request.timeout(Duration::from_secs(seconds));
        }
        // provider-native 搜索需要 beta（docs/specs/rust-websearch.md），与配置中已有的 beta 合并。
        let beta = native_search.then(|| {
            let existing = self
                .config
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("anthropic-beta"))
                .map(|(_, value)| value.as_str());
            super::web_search::merge_beta(existing)
        });
        for (key, value) in &self.config.headers {
            if beta.is_some() && key.eq_ignore_ascii_case("anthropic-beta") {
                continue;
            }
            request = request.header(key, value);
        }
        if let Some(beta) = &beta {
            request = request.header("anthropic-beta", beta);
        }
        let key = if self.config.account_access.is_some() {
            auth["requestAuth"]["apiKey"].as_str().map(str::to_owned)
        } else {
            self.config
                .api_key()
                .map_err(|_| ModelFailure::new("auth_failed", false))?
        };
        if let Some(key) = key {
            request = if self.config.api_type == ApiType::Anthropic {
                request.header("x-api-key", &key).bearer_auth(key)
            } else {
                request.bearer_auth(key)
            };
        }
        if let Some(headers) = auth["requestAuth"]["headers"].as_object() {
            for (key, value) in headers {
                request = request.header(
                    key,
                    value
                        .as_str()
                        .ok_or_else(|| ModelFailure::new("auth_failed", false))?,
                );
            }
        }
        let response = tokio::select! {
            result=request.send()=>result.map_err(|e| model_failure::network(&e))?,
            _=deadline(after(idle_ms))=>return Err(ModelFailure::new("stream_idle_timeout",true)),
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            loop {
                let chunk = tokio::select! {
                    chunk=stream.next()=>chunk,
                    _=deadline(after(idle_ms))=>return Err(ModelFailure::new("stream_idle_timeout",true)),
                };
                let Some(chunk) = chunk else {
                    break;
                };
                let chunk = chunk.map_err(|e| model_failure::network(&e))?;
                if bytes.len() + chunk.len() > 65536 {
                    break;
                }
                bytes.extend_from_slice(&chunk);
            }
            let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Err(model_failure::response(Some(status), &body, &headers));
        }
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut assembly = ProtocolStream::new(self.config.api_type);
        let mut idle_at = after(idle_ms);
        loop {
            tokio::select! {biased;
                _=deadline(output.deadline)=> {
                    let before = Instant::now();
                    output.flush().await?;
                    // stdout 背压不算供应商闲置；不能因 UI 暂停读管道误报网络故障。
                    idle_at = idle_at.and_then(|at| at.checked_add(before.elapsed()));
                },
                _=deadline(idle_at)=>return Err(ModelFailure::new("stream_idle_timeout",true)),
                chunk=stream.next()=> {
                    let Some(chunk) = chunk else { break; };
                    let chunk = chunk.map_err(|e| model_failure::network(&e))?;
                    let events = decoder.push(&chunk)?;
                    let had_event = !events.is_empty();
                    for data in events {
                        assembly.consume(&data,output).await?;
                        if assembly.done() { break; }
                    }
                    if had_event { idle_at = after(idle_ms); }
                    if assembly.done() { break; }
                },
            }
        }
        assembly.finish()
    }
    async fn complete_inner(
        &self,
        messages: Vec<Value>,
        tools: &[Value],
        sink: &EventSink,
    ) -> Result<ModelOutput> {
        let mut messages = messages;
        let has_attachments =
            super::request_attachments::materialize(&mut messages, &self.format_properties())
                .await?;
        let messages =
            super::tool_media::project(messages, self.config.api_type, &self.format_properties())?;
        let body = model_protocol::body(&self.config, messages, tools)?;
        // Bytes 克隆只增加引用计数；同一模型步骤的网络重试不再编码整段历史。
        let encoded = Bytes::from(
            serde_json::to_vec(&body).map_err(|_| ModelFailure::new("invalid_request", false))?,
        );
        let native_search = super::web_search::needs_beta(&body);
        // 大附件仅保留重试所需的已编码字节，不能在整个流期间保留多份 base64 请求树。
        drop(body);
        if encoded.len()
            > if has_attachments {
                96 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            }
        {
            return Err(ModelFailure::new("context_exceeded", false));
        }
        let mut empty_retries = 0;
        for attempt in 1..=self.retry.max_attempts {
            if attempt > 1 {
                sink.send(Event::Retry(None))
                    .await
                    .map_err(|_| ModelFailure::cancelled())?;
            }
            let mut output = TextBuffer::new(sink);
            let auth = if let Some(access) = &self.config.account_access {
                let (reply, received) = tokio::sync::oneshot::channel();
                sink.send(Event::RequestAuth {
                    provider: self.config.provider_id.clone(),
                    selection: serde_json::json!({"providerId":self.config.provider_id,"modelId":self.config.model_id,"options":{"reasoningLevel":self.config.reasoning_level}}),
                    access: access.clone(), reply,
                }).await.map_err(|_| ModelFailure::cancelled())?;
                let auth = tokio::time::timeout(Duration::from_secs(180), received)
                    .await
                    .map_err(|_| ModelFailure::new("auth_failed", false))?
                    .map_err(|_| ModelFailure::cancelled())?;
                if auth["headersApplied"] != true || !auth["requestAuth"].is_object() {
                    return Err(ModelFailure::new("auth_failed", false));
                }
                auth
            } else {
                Value::Null
            };
            let result = self
                .request(encoded.clone(), attempt, &mut output, &auth, native_search)
                .await;
            output.flush().await?;
            match result {
                Ok(mut result) => {
                    result.response_id = output.response_id().to_owned();
                    result.message["_zcode_origin"] = serde_json::json!({"provider":self.config.provider_id,"model":self.config.model_id});
                    return Ok(result);
                }
                Err(mut failure) => {
                    failure.output_committed = output.committed;
                    if !failure.retryable
                        || failure.output_committed
                        || attempt == self.retry.max_attempts
                        || (failure.empty_completion && empty_retries > 0)
                    {
                        return Err(failure);
                    }
                    if failure.empty_completion {
                        empty_retries += 1;
                    }
                    let mask = (1u64 << 53) - 1;
                    let random =
                        (uuid::Uuid::new_v4().as_u128() as u64 & mask) as f64 / mask as f64;
                    let delay_ms = self.retry.delay_ms(attempt, failure.retry_after_ms, random);
                    let reason = if failure.empty_completion {
                        "server_error"
                    } else {
                        failure.reason
                    };
                    sink.send(Event::Retry(Some(RetryState {
                        attempt,
                        max_attempts: self.retry.max_attempts,
                        next_retry_at: super::now().saturating_add(delay_ms),
                        reason_code: reason,
                    })))
                    .await
                    .map_err(|_| ModelFailure::cancelled())?;
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
        unreachable!("positive retry budget")
    }
}
fn after(ms: u64) -> Option<Instant> {
    if ms == 0 {
        None
    } else {
        Instant::now().checked_add(Duration::from_millis(ms))
    }
}
async fn deadline(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
#[async_trait::async_trait]
impl ModelPort for HttpModel {
    fn identity(&self) -> Option<crate::contract::ModelIdentity> {
        Some(crate::contract::ModelIdentity {
            provider_id: self.config.provider_id.clone(),
            model_id: self.config.model_id.clone(),
            reasoning_level: self.config.reasoning_level.clone(),
        })
    }
    fn native_web_search(&self) -> bool {
        self.config.native_web_search
    }
    fn format_properties(&self) -> Value {
        self.config.format_properties.clone().unwrap_or_else(|| serde_json::json!({"inputFormat":{"supportsText":true,"supportsImage":false,"supportsVideo":false,"supportsAudio":false,"supportsPdf":false},"outputFormat":{"supportsText":true}}))
    }
    fn with_max_output_tokens(
        &self,
        max: usize,
    ) -> anyhow::Result<Option<std::sync::Arc<dyn ModelPort>>> {
        anyhow::ensure!(max > 0, "Invalid output token limit");
        let mut config = self.config.clone();
        config.max_output_tokens = max.min(config.max_output_tokens);
        if let Some(map) = &config.max_output_map {
            let patch = crate::domain::option_map::evaluate(
                map,
                "maxOutputTokens",
                &serde_json::json!(config.max_output_tokens),
            )?;
            config.option_patches[1] = patch;
            crate::domain::option_map::validate_patches(&config.option_patches)?;
        }
        Ok(Some(std::sync::Arc::new(Self::with_pool(
            config,
            self.client.clone(),
        ))))
    }
    fn context_policy(&self) -> crate::domain::context::ContextPolicy {
        crate::domain::context::ContextPolicy {
            window: self.config.context_window,
            max_output: self.config.max_output_tokens,
            buffer: self.config.context_buffer_tokens,
            automatic: self.config.auto_compact,
        }
    }
    async fn complete(
        &self,
        messages: Vec<Value>,
        tools: &[Value],
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> Result<ModelOutput> {
        tokio::select! {biased;
            _=cancel.cancelled()=>Err(ModelFailure::cancelled()),
            result=self.complete_inner(messages,tools,sink)=>result,
        }
    }
}
