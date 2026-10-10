//! 模型用量事实（TS `recordModelUsageFact` → `model_usage` 行）：每次逻辑请求（重试后的终态）一条，
//! 字段口径照 TS——token 取归一化 usage（`inputTokens` 为含缓存的总输入），状态 completed / error / cancelled。
//! 不在会话作用域内的调用（无 sessionId）不记录：TS 的行以会话为外键。
use crate::contract::{ModelFailure, ModelOutput};
use serde_json::{Value, json};

pub(crate) fn record(
    provider_id: &str,
    model_id: &str,
    (started_ms, first_token_ms): (u64, Option<u64>),
    attempts: u32,
    result: Result<&ModelOutput, &ModelFailure>,
) {
    let scope = crate::contract::current_model_call();
    let Some(session) = scope.session_id else {
        return;
    };
    let completed_ms = super::now();
    let usage = match result {
        Ok(output) => super::model_io_project::usage(&output.usage),
        Err(_) => json!({}),
    };
    let (status, error_code, tool_calls) = match result {
        Ok(output) => ("completed", Value::Null, output.calls.len()),
        Err(failure) if failure.code == "cancelled" => ("cancelled", Value::Null, 0),
        Err(failure) => ("error", Value::from(failure.code), 0),
    };
    crate::contract::record_model_usage(json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "sessionId": session,
        "turnId": scope.turn_id,
        "querySource": scope.query_source.unwrap_or_else(|| "main_turn".into()),
        "providerId": provider_id,
        "modelId": model_id,
        "status": status,
        "startedAt": started_ms,
        "completedAt": completed_ms,
        "durationMs": completed_ms.saturating_sub(started_ms),
        "timeToFirstTokenMs": first_token_ms.map(|at| at.saturating_sub(started_ms)),
        "toolCallCount": tool_calls,
        "inputTokens": usage["inputTokens"],
        "outputTokens": usage["outputTokens"],
        "reasoningTokens": usage["reasoningTokens"],
        "cacheCreationTokens": usage["cacheWriteTokens"],
        "cacheReadTokens": usage["cacheReadTokens"],
        "providerTotalTokens": usage["totalTokens"],
        "retryCount": attempts.saturating_sub(1),
        "errorCode": error_code,
    }));
}

/// 一次逻辑请求的网络状态身份（TS createStatusContext：requestId 一次调用一个，attempt 逐次递增）。
pub(crate) struct Status<'a> {
    pub sink: &'a crate::contract::EventSink,
    pub provider_id: &'a str,
    pub model_id: &'a str,
    pub request_id: String,
    pub max_attempts: u32,
    /// 工作流 actor 本次尝试的准入票据（TS ModelRequestAdmission）：尝试开始前取，状态事件依序投入，
    /// 下一次尝试开始 / 成功 / 重试排定 / Status 释放时 drop（退避期间不持票）。
    pub ticket: tokio::sync::Mutex<Option<Box<dyn crate::contract::ModelAdmissionTicket>>>,
}

impl Status<'_> {
    /// TS ModelNetworkStatus（流式传输 `sse`）：started / completed / failed / retry_scheduled。只进遥测，发送失败忽略。
    pub(crate) async fn emit(&self, kind: &str, attempt: u32, extra: Value) {
        let mut ticket = self.ticket.lock().await;
        if kind == "model_request_started" {
            ticket.take();
            *ticket = crate::contract::acquire_model_admission(self.provider_id, self.model_id).await;
        }
        let mut status = json!({
            "type": kind, "requestId": self.request_id, "providerId": self.provider_id, "modelId": self.model_id,
            "transport": "sse", "attempt": attempt, "maxAttempts": self.max_attempts,
        });
        if let Some(source) = crate::contract::current_model_call().query_source {
            status["querySource"] = source.into();
        }
        if let (Some(target), Some(fields)) = (status.as_object_mut(), extra.as_object()) {
            target.extend(fields.clone());
        }
        if let Some(held) = ticket.as_ref() {
            held.publish(&status);
        }
        if matches!(kind, "model_request_completed" | "model_retry_scheduled") {
            ticket.take();
        }
        drop(ticket);
        let _ = self
            .sink
            .send(crate::contract::Event::ModelStatus(status))
            .await;
    }

    pub(crate) async fn settled(
        &self,
        attempt: u32,
        started_ms: u64,
        result: Result<&ModelOutput, &ModelFailure>,
    ) {
        let duration = super::now().saturating_sub(started_ms);
        match result {
            Ok(_) => {
                self.emit(
                    "model_request_completed",
                    attempt,
                    json!({ "durationMs": duration }),
                )
                .await
            }
            Err(failure) => {
                let mut extra = json!({ "durationMs": duration, "reason": failure.reason, "retryable": failure.retryable });
                if let Some(code) = failure.status_code {
                    extra["statusCode"] = code.into();
                }
                self.emit("model_request_failed", attempt, extra).await;
            }
        }
    }
}
