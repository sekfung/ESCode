//! 会话状态投影（`rust-u2-serial.md` §12.4）：`onStatus`/快照共用的 JSON 形状。

use serde_json::{Map, Value, json};

use super::signals::Signals;
use super::transport::SerialConfig;

#[derive(Clone)]
pub(super) struct Status {
    pub state: &'static str,
    pub config: Option<SerialConfig>,
    pub error: Option<(String, String)>,
}

/// TS `currentStatus`：`{state, path, signals, config?, error?, loop?}`。
/// `signals` 恒在；`config`/`error`/`loop` 只在存在时出现（TS 的缺键语义）。
pub(super) fn render_status(
    path: &str,
    status: &Status,
    signals: Signals,
    looping: Option<Value>,
) -> Value {
    let mut value = Map::new();
    value.insert("state".to_owned(), Value::from(status.state));
    value.insert("path".to_owned(), Value::from(path.to_owned()));
    value.insert("signals".to_owned(), signals.to_json());
    if let Some(config) = &status.config {
        value.insert("config".to_owned(), config.to_json());
    }
    if let Some((code, message)) = &status.error {
        value.insert(
            "error".to_owned(),
            json!({"code": code, "message": message}),
        );
    }
    if let Some(looping) = looping {
        value.insert("loop".to_owned(), looping);
    }
    Value::Object(value)
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}
