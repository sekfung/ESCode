//! WebFetch 的 HTTP 传输（reqwest + 代理解析）；插件市场 / zip 下载共用 `proxied_client`。
#[allow(unused_imports)]
use super::web_fetch::*;

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use url::Url;
use zcode_cli_domain::web_fetch as rules;

/// 应用层下载用的 reqwest 客户端：代理按 TS web-fetch 规则逐 URL 解析（显式配置、ZCODE_HTTP_PROXY、
/// ZCODE_NO_PROXY 与捕获的宿主代理），不跟随重定向，信任设置页自定义 CA。WebFetch 与插件 zip 下载共用。
pub(crate) async fn proxied_client(target: String, timeout: Duration) -> Result<reqwest::Client> {
    // 系统证书读取含阻塞 IO，放到阻塞线程构建客户端。
    tokio::task::spawn_blocking(move || -> anyhow::Result<reqwest::Client> {
        let resolution = zcode_cli_domain::net_proxy::resolve_webfetch_proxy_for_request(
            &target,
            &zcode_cli_domain::net_proxy::ProxyOptions {
                http_proxy: None,
                no_proxy: None,
                env: std::env::vars().collect(),
            },
        );
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout);
        builder = match resolution.proxy_url {
            Some(proxy) => builder.proxy(reqwest::Proxy::all(proxy)?),
            None => builder.no_proxy(),
        };
        // 设置页的自定义 CA（`ZCODE_AGENT_CA_CERT`，其次 `NODE_EXTRA_CA_CERTS`）：与模型请求同一份来源。
        for bytes in zcode_cli_host::tls_ca::extra_ca_certificates()? {
            let certificates = match reqwest::Certificate::from_pem_bundle(&bytes) {
                Ok(certificates) => certificates,
                Err(_) => vec![reqwest::Certificate::from_der(&bytes)?],
            };
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(builder.build()?)
    })
    .await?
}

/// reqwest 传输：代理按 TS web-fetch 规则逐 URL 解析，不跟随重定向，60 秒超时，10 MiB 上限。
pub(crate) struct HttpTransport;

#[async_trait]
impl Transport for HttpTransport {
    async fn get(&self, url: &Url, cancel: &CancellationToken) -> Result<Response> {
        let target = url.to_string();
        // 系统证书读取含阻塞 IO，放到阻塞线程构建客户端。
        let client = proxied_client(target, Duration::from_millis(rules::TIMEOUT_MS)).await?;
        let request = client
            .get(url.as_str())
            .header("User-Agent", rules::USER_AGENT)
            .header("Accept", rules::ACCEPT)
            .send();
        let mut response = tokio::select! {
            _ = cancel.cancelled() => bail!("Cancelled"),
            result = request => result.map_err(|e| anyhow!(request_error(&e)))?,
        };
        let status = response.status().as_u16();
        let max = rules::MAX_RESPONSE_BYTES;
        if let Some(length) = response.content_length()
            && length as usize > max
        {
            bail!("HTTP response is too large: content-length={length}, max={max}");
        }
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into(),
                )
            })
            .collect();
        let mut body = Vec::new();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => bail!("Cancelled"),
                chunk = response.chunk() => chunk.map_err(|e| anyhow!(request_error(&e)))?,
            };
            let Some(chunk) = chunk else { break };
            if body.len() + chunk.len() > max {
                bail!("HTTP response is too large: bytes>{max}");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response {
            status,
            status_text: String::new(),
            headers,
            body,
        })
    }
}

/// 把 reqwest 错误链展开到最底层原因（TS 同样暴露最深的 cause，避免只剩 "fetch failed"）。
pub(super) fn request_error(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message = format!("{message}: {text}");
        }
        source = cause.source();
    }
    message
}
