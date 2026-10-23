//! 传输层（可注入）：生产走统一出口 `escode_cli_net`（`Purpose::WebFetch`），测试用桩。

use std::collections::BTreeMap;

use async_trait::async_trait;
use url::Url;

/// 单个响应正文上限（防失控；TS 无上限，这里取远大于任何配置/场景载荷的值）。
pub const BODY_LIMIT: usize = 32 * 1024 * 1024;

pub struct ApiRequest {
    pub url: Url,
    /// 小写键。
    pub headers: BTreeMap<String, String>,
}

pub struct ApiResponse {
    /// 实际请求的 URL（改写之后）；由 `ApiClient` 填写，传输层留空。
    pub url: String,
    pub status: u16,
    /// 小写键。
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl ApiResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// `Err` = 网络层失败（消息不含 URL/凭据）。
    async fn get(&self, request: ApiRequest) -> Result<ApiResponse, String>;
}

/// 统一出口：不带凭据，不跟随重定向（3xx 以状态码返回）。
pub struct NetTransport;

#[async_trait]
impl Transport for NetTransport {
    async fn get(&self, request: ApiRequest) -> Result<ApiResponse, String> {
        let client = escode_cli_net::client(escode_cli_net::ClientOptions::new(
            escode_cli_net::Purpose::WebFetch,
            request.url.as_str(),
        ))
        .await
        .map_err(|_| "network client unavailable".to_owned())?;
        escode_cli_net::capture::record("GET", request.url.as_str());
        let mut builder = client.get(request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let mut response = builder
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().ok()?.to_owned(),
                ))
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url().to_string())?
        {
            if body.len() + chunk.len() > BODY_LIMIT {
                return Err("response body limit exceeded".to_owned());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(ApiResponse {
            url: String::new(),
            status,
            headers,
            body,
        })
    }
}
