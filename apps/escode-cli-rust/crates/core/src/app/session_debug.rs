//! `session/debug` 的运行时侧观察器（TS `session-debug.ts` 的 Rust 子集）。
//!
//! 只观察 `Event::ModelStatus`，不落盘、不参与聊天投影；快照随会话记录回收。

use std::collections::BTreeSet;

use serde_json::{Value, json};

const MAX_ROUNDS: usize = 200;
const MAX_NETWORK: usize = 100;

#[derive(Default, Clone)]
pub struct Snapshot {
    rounds: Vec<Value>,
    network: Vec<Value>,
    completed: BTreeSet<String>,
    seen: BTreeSet<String>,
    cache_input: i64,
    cache_read: i64,
    cache_requests: i64,
    cache_unknown: bool,
}

impl Snapshot {
    pub fn observe(&mut self, status: &Value) {
        let status_type = status["type"].as_str().unwrap_or_default();
        if !matches!(
            status_type,
            "model_request_started"
                | "model_request_completed"
                | "model_request_failed"
                | "model_retry_scheduled"
                | "model_stream_stalled"
        ) {
            return;
        }
        let request_id = status["requestId"].as_str().unwrap_or_default();
        let event_key = format!(
            "{request_id}:{status_type}:{}",
            status["attempt"].as_i64().unwrap_or(0)
        );
        if !self.seen.insert(event_key.clone()) {
            return;
        }
        if self.seen.len() > 2_000
            && let Some(first) = self.seen.iter().next().cloned()
        {
            self.seen.remove(&first);
        }
        let mut entry = json!({
            "eventKey": event_key,
            "traceId": status["traceId"].as_str().unwrap_or(request_id),
            "recordedAt": now_ms(),
            "statusType": status_type,
            "requestHeaders": {},
            "responseHeaders": {},
            "requestHeaderCount": 0,
            "responseHeaderCount": 0,
        });
        for key in [
            "requestId",
            "providerId",
            "modelId",
            "transport",
            "querySource",
            "queryId",
            "timestamp",
            "attempt",
            "maxAttempts",
            "nextAttempt",
            "retryable",
            "statusCode",
            "durationMs",
            "delayMs",
            "reason",
            "message",
        ] {
            if let Some(value) = status.get(key).filter(|value| !value.is_null()) {
                entry[key] = value.clone();
            }
        }
        self.network.push(entry);
        if self.network.len() > MAX_NETWORK {
            let overflow = self.network.len() - MAX_NETWORK;
            self.network.drain(0..overflow);
        }

        if status_type != "model_request_completed"
            || status["querySource"].as_str() != Some("main_turn")
            || !self.completed.insert(request_id.to_owned())
        {
            return;
        }
        let usage = &status["usage"];
        let token = |key: &str| usage[key].as_i64().filter(|value| *value >= 0);
        let input = token("inputTokens");
        let output = token("outputTokens");
        let cache_read = token("cacheReadTokens");
        let total = token("totalTokens").or_else(|| match (input, output) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
        self.cache_unknown |= input.is_none() || cache_read.is_none();
        self.cache_input += input.unwrap_or(0);
        self.cache_read += cache_read.unwrap_or(0);
        self.cache_requests += 1;
        let hit_rate = match (input, cache_read) {
            (Some(input), Some(cache)) if input > 0 => Some(cache as f64 / input as f64),
            _ => None,
        };
        let mut round_usage = serde_json::Map::new();
        for (key, value) in [
            ("inputTokens", input),
            ("outputTokens", output),
            ("totalTokens", total),
            ("reasoningTokens", token("reasoningTokens")),
            ("cachedInputTokens", cache_read),
            ("cachedWriteInputTokens", token("cacheWriteTokens")),
        ] {
            if let Some(value) = value {
                round_usage.insert(key.to_owned(), json!(value));
            }
        }
        self.rounds.push(json!({
            "eventKey": request_id,
            "requestId": request_id,
            "requestIndex": self.cache_requests,
            "recordedAt": now_ms(),
            "usage": round_usage,
            "hitRate": hit_rate,
            "generationDurationMs": Value::Null,
            "tokensPerSecond": Value::Null,
        }));
        if self.rounds.len() > MAX_ROUNDS {
            let overflow = self.rounds.len() - MAX_ROUNDS;
            self.rounds.drain(0..overflow);
        }
    }

    pub fn value(&self, session_id: &str) -> Value {
        let hit_rate = if self.cache_unknown || self.cache_input <= 0 {
            Value::Null
        } else {
            json!(self.cache_read as f64 / self.cache_input as f64)
        };
        json!({
            "sessionId": session_id,
            "rounds": self.rounds,
            "networkEntries": self.network,
            "cache": if self.cache_requests == 0 {
                Value::Null
            } else {
                json!({
                    "hitRateRequestCount": self.cache_requests,
                    "totalInputTokens": self.cache_input,
                    "totalCacheReadTokens": self.cache_read,
                    "hitRate": hit_rate,
                })
            },
        })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_main_turn_rounds_and_cache_follow_the_schema() {
        let mut snapshot = Snapshot::default();
        snapshot.observe(&json!({
            "type": "model_request_started",
            "requestId": "r1",
            "querySource": "main_turn",
            "attempt": 1,
        }));
        snapshot.observe(&json!({
            "type": "model_request_completed",
            "requestId": "r1",
            "querySource": "main_turn",
            "attempt": 1,
            "durationMs": 120,
            "usage": {"inputTokens": 100, "outputTokens": 20, "cacheReadTokens": 40},
        }));
        let value = snapshot.value("s1");
        assert_eq!(value["sessionId"], json!("s1"));
        assert_eq!(value["rounds"].as_array().unwrap().len(), 1);
        assert_eq!(value["networkEntries"].as_array().unwrap().len(), 2);
        assert_eq!(value["cache"]["totalInputTokens"], json!(100));
        assert_eq!(value["cache"]["hitRate"], json!(0.4));
    }
}
