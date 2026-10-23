//! 网络捕获（Resource Manager「网络」页）：Host 进程内所有 HTTP 出口的轻量记录。
//!
//! 与 TS `packages/shared/src/node/networkCapture.ts` 同语义：250 ms 批量、每批最多 100 条、
//! 只记 http/https/ws/wss、去掉凭据与 hash、查询值全部打码、URL 上限 2048。
//! 记录本身绝不阻塞请求路径；队列满/未开启时只增加 dropped 计数。

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const MAX_RECORDS: usize = 100;
const MAX_URL: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    timestamp: u64,
    method: String,
    url: String,
}

#[derive(Default)]
struct State {
    capture_id: Option<String>,
    records: Vec<Record>,
    dropped: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// Main 的 `networkCapture` 控制：`None` 停止并清空。
pub fn set_capture_id(capture_id: Option<String>) {
    let mut guard = state();
    match capture_id {
        Some(id) => {
            *guard = Some(State {
                capture_id: Some(id),
                ..State::default()
            });
        }
        None => *guard = None,
    }
}

/// 记录一次出站请求；未开启捕获时是 no-op。
pub fn record(method: &str, url: &str) {
    let Some(sanitized) = sanitize(url) else {
        return;
    };
    let mut guard = state();
    let Some(state) = guard.as_mut() else {
        return;
    };
    if state.records.len() >= MAX_RECORDS {
        state.dropped += 1;
        return;
    }
    state.records.push(Record {
        timestamp: now_ms(),
        method: method.chars().take(32).collect(),
        url: sanitized,
    });
}

/// 取走当前批次；无开启或无记录时返回 `None`。
pub fn flush() -> Option<Value> {
    let mut guard = state();
    let state = guard.as_mut()?;
    if state.records.is_empty() {
        return None;
    }
    let records = std::mem::take(&mut state.records);
    let dropped = std::mem::take(&mut state.dropped);
    Some(json!({
        "type": "network-capture-batch",
        "batch": {
            "captureId": state.capture_id,
            "records": records.into_iter().map(|record| json!({
                "timestamp": record.timestamp,
                "processType": "host",
                "pid": std::process::id(),
                "method": record.method,
                "url": record.url,
            })).collect::<Vec<_>>(),
            "dropped": dropped,
        },
    }))
}

fn sanitize(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https" | "ws" | "wss") {
        return None;
    }
    let mut url = parsed;
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    if url.query().is_some() {
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, _)| (key.into_owned(), "redacted".to_owned()))
            .collect();
        if !pairs.is_empty() {
            url.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }
    let mut text = url.to_string();
    if text.len() > MAX_URL {
        text.truncate(MAX_URL);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_credentials_hash_and_query_values() {
        let url = sanitize("https://user:pass@example.com/a?token=secret#frag").expect("url");
        assert_eq!(url, "https://example.com/a?token=redacted");
        assert!(sanitize("file:///tmp/x").is_none());
    }

    #[test]
    fn flush_requires_an_active_capture_and_takes_records() {
        set_capture_id(None);
        record("GET", "https://example.com/");
        assert!(flush().is_none());
        set_capture_id(Some("cap-1".to_owned()));
        record("GET", "https://example.com/");
        let batch = flush().expect("batch");
        assert_eq!(batch["batch"]["captureId"], "cap-1");
        assert_eq!(batch["batch"]["records"].as_array().map(Vec::len), Some(1));
        assert!(flush().is_none());
        set_capture_id(None);
    }
}
