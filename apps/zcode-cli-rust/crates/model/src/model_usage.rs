//! 模型用量事实（TS `recordModelUsageFact` → `model_usage` 行）：每次逻辑请求（重试后的终态）一条，
//! 字段口径照 TS——token 取归一化 usage（`inputTokens` 为含缓存的总输入），状态 completed / error / cancelled。
//! 不在会话作用域内的调用（无 sessionId）不记录：TS 的行以会话为外键。
use crate::contract::{ModelFailure, ModelOutput};
use serde_json::{Value, json};

pub(crate) fn record(
    provider_id: &str,
    model_id: &str,
    started_ms: u64,
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
