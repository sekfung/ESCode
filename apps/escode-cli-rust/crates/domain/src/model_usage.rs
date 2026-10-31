//! 模型用量 → 上下文用量（TS `packages/contracts/src/model/index.ts` 的
//! `getModelUsageContextTokens` / `getModelUsageInputWindowTokens`），供 V4 `usage.contextWindow` 投影使用。
//!
//! 只认供应商回报的 usage；本地 chars→tokens 估算只用于压缩判定，不能混进 wire
//! （docs/specs/rust-context-management.md「V4 usage.contextWindow 投影」）。
use serde_json::Value;

use crate::session::Session;

/// 模型选择事件带来的窗口容量；缺失时保持未知（协议字段写 `null`）。
pub fn capacity(usage: &Value) -> Option<u64> {
    usage["maxTokens"].as_u64()
}

/// 上下文 token 数：输入的窗口部分 + 输出；输入缺失时退 `total - output`，再退 cache 读写之和。
pub fn context_tokens(usage: &Value) -> Option<u64> {
    let output = usage["completion_tokens"].as_u64().unwrap_or(0);
    let positive = |value: &Value| value.as_u64().filter(|tokens| *tokens > 0);
    let input = positive(&usage["prompt_tokens"])
        .or_else(|| positive(&usage["total_tokens"]).map(|total| total.saturating_sub(output)));
    let cache_write = usage["prompt_tokens_details"]["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache = usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(cache_write);
    let input = input.or(Some(cache).filter(|tokens| *tokens > 0));
    let used = input.unwrap_or(0).saturating_add(output);
    if used > 0 {
        Some(used)
    } else {
        positive(&usage["total_tokens"])
    }
}

/// Node `onModelComplete` / `onCompactCompleted` 共用的 `usage.contextWindow` 投影。
///
/// `maxTokens` 优先沿用快照已有容量，其次用本轮模型选择记录的容量；都未知时整个字段为 `null`。
/// `autoCompactThresholdTokens` 沿用已有值（Node 初始为 `null`），cache/breakdown 等诊断块不保留。
pub fn write_context_window(session: &mut Session, used_tokens: u64) {
    let max = session.usage["contextWindow"]["maxTokens"]
        .as_u64()
        .or(session.context_window_max_tokens);
    let threshold = session.usage["contextWindow"]["autoCompactThresholdTokens"].clone();
    session.usage["contextWindow"] = match max {
        Some(max) => serde_json::json!({
            "usedTokens": used_tokens,
            "maxTokens": max,
            "autoCompactThresholdTokens": threshold,
        }),
        None => Value::Null,
    };
}

/// TS `decodeContribution`：把单次调用的解码区间折算成 `turnHeader` 的增量。
/// `decodeMs` 缺失或为零跨度时该次调用不贡献；token 数取 `outputTokens - 1`（跨度覆盖 N−1 个间隔）。
pub fn decode_contribution(decode_ms: Option<u64>, output_tokens: u64) -> Option<(u64, i64)> {
    let decode_ms = decode_ms.filter(|ms| *ms > 0)?;
    Some((decode_ms, output_tokens as i64 - 1))
}

#[cfg(test)]
mod tests {
    use super::{capacity, context_tokens, decode_contribution, write_context_window};
    use crate::session::Session;
    use serde_json::{Value, json};

    #[test]
    fn input_plus_output_wins() {
        assert_eq!(
            context_tokens(&json!({"prompt_tokens": 1000, "completion_tokens": 50})),
            Some(1050)
        );
    }

    #[test]
    fn falls_back_to_total_minus_output() {
        assert_eq!(
            context_tokens(&json!({"total_tokens": 1050, "completion_tokens": 50})),
            Some(1050)
        );
    }

    #[test]
    fn falls_back_to_cache_tokens_then_total() {
        assert_eq!(
            context_tokens(&json!({
                "prompt_tokens_details": {"cached_tokens": 200, "cache_write_tokens": 10},
                "completion_tokens": 5
            })),
            Some(215)
        );
        assert_eq!(context_tokens(&json!({"total_tokens": 42})), Some(42));
    }

    #[test]
    fn missing_usage_stays_unknown() {
        assert_eq!(context_tokens(&json!({})), None);
        assert_eq!(
            context_tokens(&json!({"prompt_tokens": 0, "completion_tokens": 0})),
            None
        );
    }

    fn session() -> Session {
        Session::new(
            "sess_test".into(),
            "workspace".into(),
            "provider".into(),
            "model".into(),
            "medium".into(),
            "epoch".into(),
            0,
        )
    }

    #[test]
    fn new_session_keeps_context_window_null_before_any_model_completion() {
        let session = session();
        assert_eq!(session.usage["contextWindow"], Value::Null);
        assert_eq!(session.context_window_max_tokens, None);
        assert_eq!(capacity(&json!({"maxTokens": 256000})), Some(256000));
        assert_eq!(capacity(&json!({"maxTokens": "unknown"})), None);
    }

    #[test]
    fn write_uses_capacity_from_model_selection() {
        let mut session = session();
        session.context_window_max_tokens = Some(256_000);
        write_context_window(&mut session, 1340);
        assert_eq!(
            session.usage["contextWindow"],
            json!({"usedTokens":1340,"maxTokens":256000,"autoCompactThresholdTokens":null})
        );
    }

    #[test]
    fn write_preserves_threshold_and_drops_diagnostics() {
        let mut session = session();
        session.usage["contextWindow"] = json!({
            "usedTokens": 9000,
            "maxTokens": 200000,
            "autoCompactThresholdTokens": 160000,
            "cache": {"readTokens": 20},
            "breakdown": [{"label": "system", "tokens": 10}],
        });
        write_context_window(&mut session, 42);
        assert_eq!(
            session.usage["contextWindow"],
            json!({"usedTokens":42,"maxTokens":200000,"autoCompactThresholdTokens":160000})
        );
    }

    #[test]
    fn write_with_unknown_capacity_stays_null() {
        let mut session = session();
        write_context_window(&mut session, 42);
        assert_eq!(session.usage["contextWindow"], Value::Null);
    }

    #[test]
    fn decode_contribution_matches_node_and_drops_zero_spans() {
        assert_eq!(decode_contribution(None, 11), None);
        assert_eq!(decode_contribution(Some(0), 11), None);
        assert_eq!(decode_contribution(Some(500), 11), Some((500, 10)));
        // 单 token 的区间没有可计量的 token 间隔；Node 同样给出 0。
        assert_eq!(decode_contribution(Some(120), 1), Some((120, 0)));
    }
}
