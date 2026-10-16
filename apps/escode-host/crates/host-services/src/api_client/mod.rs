//! Host 自己的 ApiClient（docs/specs/rust-u2-client-config.md §3；TS `providers/api/nodeApiClient.ts` + `apiJson.ts`）。
//!
//! 只做 GET（两个调用方 `client-config`/`client-scenes` 都只读）：Endpoint 改写 → 来源头 → `x-request-id` → 超时 → 传输。
//! JWT/业务 401 观察器随 OAuth 删除而无调用方，不迁。

mod error;
mod source_headers;
mod transport;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use url::Url;

pub use error::ApiError;
pub use source_headers::{ClientIdentity, normalize_value};
pub use transport::{ApiRequest, ApiResponse, NetTransport, Transport};

use crate::provider::{DEFAULT_ENDPOINT_ORIGIN, EndpointSource, normalize_endpoint_origin};

/// TS `rewriteESCodeEndpointUrl`：默认 Origin 的 URL 改写到当前 Endpoint（保留 path/search/hash）；其余原样。
pub fn rewrite_endpoint_url(input: &str, endpoint_origin: &str) -> String {
    let Ok(parsed) = Url::parse(input) else {
        return input.to_owned();
    };
    if parsed.origin().ascii_serialization() != DEFAULT_ENDPOINT_ORIGIN {
        return input.to_owned();
    }
    let Ok(target_origin) = normalize_endpoint_origin(endpoint_origin) else {
        return input.to_owned();
    };
    if target_origin == DEFAULT_ENDPOINT_ORIGIN {
        return input.to_owned();
    }
    let Ok(mut target) = Url::parse(&target_origin) else {
        return input.to_owned();
    };
    target.set_path(parsed.path());
    target.set_query(parsed.query());
    target.set_fragment(parsed.fragment());
    target.to_string()
}

#[derive(Default, Clone)]
pub struct GetOptions {
    /// `> 0` 才计时；超时错误文案 `Request timed out after <n>ms`。
    pub timeout: Option<Duration>,
    /// 调用方头（小写键），优先于来源头。
    pub headers: BTreeMap<String, String>,
}

pub struct ApiClient {
    endpoint: EndpointSource,
    identity: ClientIdentity,
    transport: Arc<dyn Transport>,
}

impl ApiClient {
    pub fn new(
        endpoint: EndpointSource,
        identity: ClientIdentity,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            endpoint,
            identity,
            transport,
        }
    }

    /// 当前 Endpoint（每次解析；设置里的覆盖值随时可变）。
    pub async fn endpoint_origin(&self) -> Result<String, ApiError> {
        self.endpoint
            .resolve()
            .await
            .map_err(|error| ApiError::network(format!("{error:#}"), ""))
    }

    pub async fn get(&self, url: &str, options: &GetOptions) -> Result<ApiResponse, ApiError> {
        let endpoint_origin = self.endpoint_origin().await?;
        let request_url = rewrite_endpoint_url(url, &endpoint_origin);
        let parsed = Url::parse(&request_url)
            .map_err(|error| ApiError::network(error.to_string(), &request_url))?;

        let mut headers = BTreeMap::new();
        // 来源头只给发往当前 Endpoint 的请求（TS `isRequestForEndpoint`）。
        let endpoint = normalize_endpoint_origin(&endpoint_origin).ok();
        if endpoint.as_deref() == Some(parsed.origin().ascii_serialization().as_str()) {
            headers = self.identity.headers().await;
            if headers.get("http-referer").map(String::as_str) == Some(DEFAULT_ENDPOINT_ORIGIN) {
                // 先让调用方头覆盖，再把仍是默认值的 Referer 换成当前 Endpoint（与 TS 顺序一致）。
                headers.extend(options.headers.clone());
                if headers.get("http-referer").map(String::as_str) == Some(DEFAULT_ENDPOINT_ORIGIN)
                {
                    headers.insert("http-referer".to_owned(), endpoint_origin.clone());
                }
            } else {
                headers.extend(options.headers.clone());
            }
        } else {
            headers.extend(options.headers.clone());
        }
        headers
            .entry("x-request-id".to_owned())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string());

        let send = self.transport.get(ApiRequest {
            url: parsed,
            headers,
        });
        let result = match options.timeout.filter(|timeout| !timeout.is_zero()) {
            Some(timeout) => tokio::time::timeout(timeout, send).await.map_err(|_| {
                ApiError::network(
                    format!("Request timed out after {}ms", timeout.as_millis()),
                    &request_url,
                )
            })?,
            None => send.await,
        };
        let mut response = result.map_err(|message| ApiError::network(message, &request_url))?;
        response.url = request_url;
        Ok(response)
    }

    /// TS `readApiJson`：非 2xx → 带状态码与诊断头的 `ApiError`；JSON 解析失败同。
    pub async fn get_json(&self, url: &str, options: &GetOptions) -> Result<Value, ApiError> {
        let response = self.get(url, options).await?;
        let rewritten = rewrite_endpoint_url(url, &self.endpoint_origin().await?);
        let describe = |message: String| ApiError {
            message,
            url: rewritten.clone(),
            method: "GET".to_owned(),
            status: Some(response.status),
            response_headers: error::diagnostic_headers(&response.headers),
        };
        if !response.ok() {
            return Err(describe(error::response_message(
                &response.body,
                response.status,
            )));
        }
        serde_json::from_slice(&response.body).map_err(|error| describe(error.to_string()))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
