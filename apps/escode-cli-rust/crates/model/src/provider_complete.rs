//! HttpModel 的完整调用循环（从 provider.rs 拆出）：重试、空响应二次重试、鉴权冻结与取消。

use super::provider::HttpModel;
use super::{model_protocol, model_stream::TextBuffer};
use crate::contract::{Event, EventSink, ModelFailure, ModelOutput, ModelPort, RetryState};
use bytes::Bytes;
use serde_json::Value;
use std::time::Duration;

type Result<T> = std::result::Result<T, ModelFailure>;

impl HttpModel {
    pub(super) async fn complete_inner(
        &self,
        messages: Vec<Value>,
        tools: &[Value],
        sink: &EventSink,
    ) -> Result<ModelOutput> {
        // model-IO 记录（docs/specs/rust-model-io.md）：投影在展开附件之前做；test 环境为 None。
        let io = super::model_io::Call::new(
            &self.config.provider_id,
            &self.config.model_id,
            self.config.max_output_tokens,
            &messages,
            tools,
        );
        let mut messages = messages;
        let has_attachments =
            super::request_attachments::materialize(&mut messages, &self.format_properties())
                .await?;
        let mut messages =
            super::tool_media::project(messages, self.config.api_type, &self.format_properties())?;
        // 历史里的 tool_reference 只有在 native 请求中有线上意义；切换模式/provider 时降级为文本。
        let native_tool_search = tools
            .iter()
            .any(|tool| tool["function"]["deferLoading"] == true);
        super::tool_reference::project(&mut messages, native_tool_search);
        let body = model_protocol::body(&self.config, messages, tools)?;
        // Bytes 克隆只增加引用计数；同一模型步骤的网络重试不再编码整段历史。
        // MCP 工具 schema 按登记的声明顺序编码（docs/specs/rust-tool-schema-order.md），其余与 serde_json 相同。
        let encoded = Bytes::from(
            crate::domain::schema_order::encode(&body, crate::domain::schema_order::lookup)
                .into_bytes(),
        );
        let native_search = super::web_search::needs_beta(&body);
        let tool_search = super::tool_search::needs_beta_body(&body);
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
        let request_started = super::now();
        let status = super::model_usage::Status {
            sink,
            provider_id: &self.config.provider_id,
            model_id: &self.config.model_id,
            request_id: uuid::Uuid::new_v4().to_string(),
            max_attempts: self.retry.max_attempts,
            ticket: Default::default(),
        };
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
            let (started, attempt_started) = (std::time::SystemTime::now(), super::now());
            status
                .emit("model_request_started", attempt, serde_json::json!({}))
                .await;
            let result = self
                .request(
                    encoded.clone(),
                    attempt,
                    &mut output,
                    &auth,
                    native_search,
                    tool_search,
                )
                .await;
            output.flush().await?;
            let result = result.map(|mut result| {
                result.response_id = output.response_id().to_owned();
                result
            });
            if let Some(io) = &io {
                io.record(attempt, started, &encoded, result.as_ref()).await;
            }
            status
                .settled(attempt, attempt_started, result.as_ref())
                .await;
            match result {
                Ok(mut result) => {
                    super::model_usage::record(
                        status.provider_id,
                        status.model_id,
                        (request_started, output.first_token_at),
                        attempt,
                        Ok(&result),
                    );
                    result.message["_escode_origin"] = serde_json::json!({"provider":self.config.provider_id,"model":self.config.model_id});
                    return Ok(result);
                }
                Err(mut failure) => {
                    failure.output_committed = output.committed;
                    if !failure.retryable
                        || failure.output_committed
                        || attempt == self.retry.max_attempts
                        || (failure.empty_completion && empty_retries > 0)
                    {
                        super::model_usage::record(
                            status.provider_id,
                            status.model_id,
                            (request_started, output.first_token_at),
                            attempt,
                            Err(&failure),
                        );
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
                    status.emit("model_retry_scheduled", attempt, serde_json::json!({ "delayMs": delay_ms, "nextAttempt": attempt + 1, "reason": reason })).await;
                    sink.send(Event::Retry(Some(RetryState {
                        attempt,
                        max_attempts: self.retry.max_attempts,
                        next_retry_at: super::now().saturating_add(delay_ms),
                        reason_code: escode_cli_domain::model::retry_reason_code(reason),
                        reason,
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
