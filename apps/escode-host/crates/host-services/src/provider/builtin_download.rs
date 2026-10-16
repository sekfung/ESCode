//! Built-in Release 下载（TS `escode-builtin-download.ts`，spec §12）。
//!
//! 请求通过 `JsonGet` 注入：生产用统一出口（`HttpGet`），测试用桩。错误文案不含 URL/query/正文
//! （它们可能带凭据或路径），只保留 stage 与简短原因。

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use super::files;

/// 两次请求共用的总预算（TS 20 s）。
pub const TOTAL_BUDGET: Duration = Duration::from_secs(20);
/// 单个响应正文上限（TS 10,000,000 字节）。
pub const BODY_LIMIT: usize = 10_000_000;

/// 取一个 JSON 文档；`Err` 是不含敏感信息的简短原因（`HTTP 500`、`body limit exceeded`……）。
#[async_trait]
pub trait JsonGet: Send + Sync {
    async fn get_json(&self, url: &Url) -> Result<Value, String>;
}

/// 无 Release（Endpoint 未配置 `builtin_provider_config_json`）返回 `Ok(None)`。
pub async fn download_release(
    endpoint_origin: &str,
    app_version: &str,
    platform: &str,
    get: &dyn JsonGet,
    budget: Duration,
) -> Result<Option<Value>, String> {
    let stage = std::sync::atomic::AtomicBool::new(false);
    let run = async {
        let mut url = Url::parse(endpoint_origin).map_err(|_| "invalid response".to_owned())?;
        url.set_path("/api/v1/client/configs");
        url.query_pairs_mut()
            .append_pair("app_version", app_version)
            .append_pair("platform", platform);
        let payload = get.get_json(&url).await?;
        let Some(download) = download_url(&payload)? else {
            return Ok(None);
        };
        stage.store(true, std::sync::atomic::Ordering::Relaxed);
        let release = get.get_json(&download).await?;
        // 完整的 Release 校验（含“禁含已退出的 Provider”）；细节不外露。
        files::parse_builtin(&release).map_err(|_| "invalid response".to_owned())?;
        Ok(Some(release))
    };
    let result = match tokio::time::timeout(budget, run).await {
        Ok(result) => result,
        Err(_) => Err("timeout".to_owned()),
    };
    result.map_err(|reason| {
        let stage = if stage.load(std::sync::atomic::Ordering::Relaxed) {
            "cdn"
        } else {
            "client-config"
        };
        format!("ESCode Built-in {stage}: {reason}")
    })
}

/// TS `clientConfigSchema`：`{code:0, data:{configs:{builtin_provider_config_json?: https URL，无 userinfo}}}`。
fn download_url(payload: &Value) -> Result<Option<Url>, String> {
    let invalid = || "invalid response".to_owned();
    if payload.get("code").and_then(Value::as_i64) != Some(0) {
        return Err(invalid());
    }
    let configs = payload
        .get("data")
        .and_then(|data| data.get("configs"))
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let Some(value) = configs.get("builtin_provider_config_json") else {
        return Ok(None);
    };
    let url = value
        .as_str()
        .and_then(|text| Url::parse(text).ok())
        .ok_or_else(invalid)?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    Ok(Some(url))
}

/// 统一出口（`escode_cli_net`，与 `system_probe` 同一入口）：无凭据；客户端不跟随重定向，3xx 即 `HTTP 3xx`。
pub struct HttpGet;

#[async_trait]
impl JsonGet for HttpGet {
    async fn get_json(&self, url: &Url) -> Result<Value, String> {
        let client = escode_cli_net::client(escode_cli_net::ClientOptions::new(
            escode_cli_net::Purpose::WebFetch,
            url.as_str(),
        ))
        .await
        .map_err(|_| "network client unavailable".to_owned())?;
        let mut response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| "request failed".to_owned())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "request failed".to_owned())?
        {
            if body.len() + chunk.len() > BODY_LIMIT {
                return Err("body limit exceeded".to_owned());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "invalid response".to_owned())
    }
}

#[cfg(test)]
#[path = "builtin_download_tests.rs"]
mod tests;
