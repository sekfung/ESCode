//! TS `ApiError` + `readApiJson` 的错误文案抽取（`providers/api/apiJson.ts`）。

use std::collections::BTreeMap;

use serde_json::Value;

/// 诊断用响应头（TS `DIAGNOSTIC_RESPONSE_HEADER_NAMES`）。
const DIAGNOSTIC_HEADERS: [&str; 3] = ["x-request-id", "x-trace-id", "x-span-id"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    /// 只含 [`DIAGNOSTIC_HEADERS`] 里非空的那几个。
    pub response_headers: BTreeMap<String, String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    pub fn network(message: impl Into<String>, url: &str) -> Self {
        Self {
            message: message.into(),
            url: url.to_owned(),
            method: "GET".to_owned(),
            status: None,
            response_headers: BTreeMap::new(),
        }
    }
}

pub(super) fn diagnostic_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    DIAGNOSTIC_HEADERS
        .iter()
        .filter_map(|name| {
            let value = headers.get(*name)?.trim();
            (!value.is_empty()).then(|| ((*name).to_owned(), value.to_owned()))
        })
        .collect()
}

/// TS `readResponseMessage`：JSON 正文取 `error|message|msg|detail` 的首个**字符串**（trim 后为空则 `HTTP <status>`），
/// 非 JSON 返回 trim 后的正文，空正文 `HTTP <status>`。
pub(super) fn response_message(body: &[u8], status: u16) -> String {
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return format!("HTTP {status}");
    }
    match serde_json::from_str::<Value>(trimmed) {
        // JS：`null.error` 抛 TypeError，被内层 catch 吞掉后回落到正文本身。
        Ok(Value::Null) | Err(_) => trimmed.to_owned(),
        Ok(parsed) => {
            let picked = ["error", "message", "msg", "detail"]
                .iter()
                .find_map(|key| parsed.get(*key).and_then(Value::as_str))
                .map_or("", str::trim);
            if picked.is_empty() {
                format!("HTTP {status}")
            } else {
                picked.to_owned()
            }
        }
    }
}
