//! 断流恢复（docs/specs/rust-model-retry.md「断流恢复」）：模型已流出正文或推理后失败，适配层不再重试；
//! 对齐 TS `recoverPartialAssistantOutputFailure`（no_tool_committed 路径）：作废本次 assistant 尾部（行收口为
//! interrupted、不进历史），从同一安全锚点（失败前的历史）重新请求，每个 run 至多 10 次。
//! Rust 在模型完成后才执行工具，不存在「部分工具已提交」的恢复路径。
use crate::contract::{Event, EventSink, ModelFailure};
use anyhow::Result;

/// TS `STREAM_RECOVERY_MAX_RETRIES`。
pub(super) const MAX_RETRIES: u32 = 10;

/// TS `TRANSIENT_ERROR_REASONS`。
const TRANSIENT_REASONS: [&str; 5] = [
    "stream_idle_timeout",
    "rate_limited",
    "server_error",
    "network_error",
    "timeout",
];

/// 可恢复：已有可见输出（适配层不会再重试），且失败可重试或属于瞬时原因（TS isRetryableStreamRecoveryFailure）。
pub(super) fn recoverable(failure: &ModelFailure) -> bool {
    failure.output_committed
        && failure.reason != "cancelled"
        && (failure.retryable || TRANSIENT_REASONS.contains(&failure.reason))
}

/// TS `classifyStreamRecoveryFailure` + product-projection `streamRecoveryReasonCode`。
fn reason_code(failure: &ModelFailure) -> &'static str {
    match failure.reason {
        "stream_idle_timeout" | "timeout" => "fault.network.timeout",
        "network_error" => "fault.network.unreachable",
        _ => "fault.network.sseDisconnected",
    }
}

/// 通知 owner 作废失败尾部并显示恢复态；随后由调用方用未变的历史重发请求。
pub(super) async fn start(sink: &EventSink, retry_number: u32, failure: &ModelFailure) -> Result<()> {
    sink.send(Event::StreamRecovery {
        retry_number,
        max_retries: MAX_RETRIES,
        reason_code: reason_code(failure),
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(reason: &'static str, retryable: bool, committed: bool) -> ModelFailure {
        ModelFailure {
            output_committed: committed,
            ..ModelFailure::new(reason, retryable)
        }
    }

    #[test]
    fn only_committed_transient_failures_recover() {
        assert!(recoverable(&failure("network_error", true, true)));
        assert!(recoverable(&failure("stream_idle_timeout", false, true)));
        assert!(!recoverable(&failure("network_error", true, false)));
        assert!(!recoverable(&failure("cancelled", false, true)));
        assert!(!recoverable(&failure("invalid_request", false, true)));
    }

    #[test]
    fn reason_codes_follow_ts_failure_kinds() {
        assert_eq!(reason_code(&failure("stream_idle_timeout", true, true)), "fault.network.timeout");
        assert_eq!(reason_code(&failure("network_error", true, true)), "fault.network.unreachable");
        assert_eq!(reason_code(&failure("server_error", true, true)), "fault.network.sseDisconnected");
    }
}
