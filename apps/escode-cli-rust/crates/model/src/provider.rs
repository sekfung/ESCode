use super::{
    config::ModelConfig,
    model_failure,
    model_policy::RetryPolicy,
    model_protocol::{ApiType, ProtocolStream},
    model_stream::TextBuffer,
    sse::SseDecoder,
};
use crate::contract::{EventSink, ModelFailure, ModelOutput, ModelPort};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub(super) use super::provider_support::{after, deadline};

type Result<T> = std::result::Result<T, ModelFailure>;

pub struct HttpModel {
    pub(super) config: ModelConfig,
    client: std::sync::Arc<tokio::sync::OnceCell<reqwest::Client>>,
    pub(super) retry: RetryPolicy,
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
                // 出口策略（代理 + 自定义 CA）统一在 crates/net；证书读取在该 crate 的
                // 受管阻塞线程内完成，不阻塞 stdio actor 的启动与取消（rust-unified-host.md P1 #6）。
                escode_cli_net::client(
                    escode_cli_net::ClientOptions::new(escode_cli_net::Purpose::Model, &self.url)
                        .pool_idle(Duration::from_secs(90))
                        .tcp_nodelay(),
                )
                .await
                .map_err(|error| {
                    // net 的失败可能是 reqwest 错误（代理/CA/构建）或环境解析错误：
                    // reqwest 来源按网络故障分类，其余按无效请求（与原先的 spawn_blocking
                    // 分支同一口径：join 失败 → invalid_request）。
                    match error.downcast_ref::<reqwest::Error>() {
                        Some(inner) => model_failure::network(inner),
                        None => ModelFailure::new("invalid_request", false),
                    }
                })
            })
            .await
    }
    pub(super) async fn request(
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
                .map(|(_, value)| value.expose());
            super::web_search::merge_beta(existing)
        });
        for (key, value) in
            super::client_headers::request_headers(&self.config.headers, beta.is_some(), &self.url)
        {
            request = request.header(key, value);
        }
        if let Some(beta) = &beta {
            request = request.header("anthropic-beta", beta);
        }
        let key = if self.config.account_access.is_some() {
            auth["requestAuth"]["apiKey"]
                .as_str()
                .map(escode_cli_core_api::RedactedString::new)
        } else {
            self.config
                .api_key()
                .map_err(|_| ModelFailure::new("auth_failed", false))?
        };
        if let Some(key) = key {
            let key = key.expose();
            request = if self.config.api_type == ApiType::Anthropic {
                request.header("x-api-key", key).bearer_auth(key)
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
        escode_cli_net::capture::record("POST", &self.url);
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
