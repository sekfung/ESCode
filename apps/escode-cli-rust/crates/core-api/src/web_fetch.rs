use serde_json::Value;

/// WebFetch 抓取结果。`content` 为 None 时 `output` 是完整的终态 WebFetchOutput（重定向 / HTTP 错误）；
/// 否则 `output` 缺 result / truncated / durationMs，由会话侧处理正文后补齐。
#[derive(Debug)]
pub struct WebFetchPage {
    pub output: Value,
    pub content: Option<String>,
    pub preapproved: bool,
}
