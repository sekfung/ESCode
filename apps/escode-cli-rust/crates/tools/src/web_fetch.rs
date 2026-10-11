//! WebFetch 抓取阶段：URL 规范化、逐跳出网拦截、手动重定向、HTTP 错误与重定向终态文案、正文抽取与进程级缓存。
//! 逐条对齐 TS `webfetch.ts` / `webfetch-network.ts` / `webfetch-cache.ts`，见 docs/specs/rust-webfetch.md。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;
use escode_cli_domain::web_fetch as rules;

use crate::contract::WebFetchPage;

#[allow(unused_imports)]
pub(super) use super::web_fetch_http::{HttpTransport, proxied_client};

const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

const CACHE_MAX_BYTES: usize = 50 * 1024 * 1024;

pub(crate) struct Response {
    pub status: u16,
    pub status_text: String,
    /// 名称已转小写。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[async_trait]
pub(crate) trait Transport: Send + Sync {
    /// 单次 GET，不跟随重定向；失败时返回可直接给模型的文案。
    async fn get(&self, url: &Url, cancel: &CancellationToken) -> Result<Response>;
}

/// 超预算正文的 artifact 目标（TS `maybePersistRawContent` + `writeToolResultArtifact`）：
/// `<root>/<session>/<toolCallId>-tool-result-<uuid><ext>`，URI `escode-artifact://<session>/tool-result-<uuid>`。
#[derive(Clone, Copy)]
pub(crate) struct ArtifactTarget<'a> {
    pub root: &'a std::path::Path,
    pub session: &'a str,
    pub call_id: &'a str,
}

impl ArtifactTarget<'_> {
    /// 正文（抽取后的 markdown/文本）按 UTF-8 字节数超过 100k 时落盘；否则返回 (None, None)。
    /// artifact 的 contentType 与 TS 相同：响应含 `html` 记 `text/markdown`（`.md`），其余 `text/plain`（`.txt`）。
    async fn write(
        &self,
        content: &str,
        content_type: &str,
    ) -> Result<(Option<String>, Option<String>)> {
        if content.len() <= rules::MAX_MODEL_INPUT_CHARS {
            return Ok((None, None));
        }
        let extension = if content_type.contains("html") {
            ".md"
        } else {
            ".txt"
        };
        let artifact = format!("tool-result-{}", escode_cli_host::id());
        let directory = self
            .root
            .join(super::mcp_connection::sanitize(self.session));
        let path = directory.join(format!(
            "{}-{artifact}{extension}",
            super::mcp_connection::sanitize(self.call_id)
        ));
        tokio::fs::create_dir_all(&directory).await?;
        tokio::fs::write(&path, content).await?;
        Ok((
            Some(path.to_string_lossy().into_owned()),
            Some(format!("escode-artifact://{}/{artifact}", self.session)),
        ))
    }
}

#[derive(Clone)]
struct Cached {
    bytes: usize,
    content: String,
    content_type: String,
    final_url: String,
    redirects: Vec<Value>,
    status: u16,
    status_text: String,
    /// 超预算正文的 artifact（TS `maybePersistRawContent`）：写一次、缓存复用，命中缓存不重复落盘。
    artifact_path: Option<String>,
    artifact_uri: Option<String>,
}

enum Fetched {
    Content(Cached),
    Terminal(Value),
}

pub(crate) async fn fetch(
    transport: &dyn Transport,
    args: &Value,
    target: Option<ArtifactTarget<'_>>,
    cancel: &CancellationToken,
) -> Result<WebFetchPage> {
    super::tools::keys(args, &["url", "prompt"])?;
    let url = super::tools::string(args, "url")?.to_owned();
    let prompt = super::tools::string(args, "prompt")?.to_owned();
    let started = Instant::now();
    if let Some(message) = rules::zod_url_error(&url) {
        return Err(anyhow!(message));
    }
    let normalized = rules::normalize_url(&url).map_err(|message| anyhow!(message))?;
    let preapproved = rules::is_preapproved(&url);
    let cached = cache_get(&url);
    let cache_hit = cached.is_some();
    let fetched = match cached {
        Some(entry) => entry,
        None => match fetch_fresh(transport, &url, normalized, &prompt, target, cancel).await? {
            Fetched::Terminal(mut output) => {
                output["durationMs"] = json!(started.elapsed().as_millis() as u64);
                return Ok(WebFetchPage {
                    output,
                    content: None,
                    preapproved,
                });
            }
            Fetched::Content(entry) => {
                cache_put(&url, entry.clone());
                entry
            }
        },
    };
    let mut output = json!({
        "url": url,
        "finalUrl": fetched.final_url,
        "status": fetched.status,
        "statusText": status_text(fetched.status, &fetched.status_text),
        "contentType": fetched.content_type,
        "bytes": fetched.bytes,
        "cacheHit": cache_hit,
        "redirects": fetched.redirects,
    });
    // 与 TS `WebFetchOutput` 一样只在有 artifact 时带上这两个键（undefined 会被 JSON 序列化丢掉）。
    if let Some(uri) = &fetched.artifact_uri {
        output["artifactUri"] = uri.clone().into();
    }
    if let Some(path) = &fetched.artifact_path {
        output["artifactPath"] = path.clone().into();
    }
    Ok(WebFetchPage {
        output,
        content: Some(fetched.content),
        preapproved,
    })
}

async fn fetch_fresh(
    transport: &dyn Transport,
    original: &str,
    mut current: Url,
    prompt: &str,
    target: Option<ArtifactTarget<'_>>,
    cancel: &CancellationToken,
) -> Result<Fetched> {
    let mut redirects: Vec<Value> = Vec::new();
    let mut response = None;
    for _ in 0..=rules::MAX_REDIRECTS {
        // 每次真实 GET 前阻断字面量本地/私网目标，避免 NO_PROXY 绕过边界。
        if let Some(message) = rules::egress_block(&current) {
            bail!(message);
        }
        let reply = transport.get(&current, cancel).await?;
        if !is_redirect(reply.status) {
            response = Some(reply);
            break;
        }
        let Some(location) = header(&reply, "location").filter(|l| !l.trim().is_empty()) else {
            return Ok(Fetched::Terminal(http_error(
                original, &current, &redirects, &reply,
            )));
        };
        let next = rules::resolve_redirect(location, &current).map_err(|m| anyhow!(m))?;
        let shown = rules::redact_credentials(&next);
        let hop = json!({"from": current.to_string(), "to": shown, "status": reply.status});
        if !rules::redirect_permitted(&current, &next) {
            redirects.push(hop);
            return Ok(Fetched::Terminal(redirect_output(
                original, &current, &shown, &redirects, &reply, prompt,
            )));
        }
        redirects.push(hop);
        current = next;
        response = Some(reply);
    }
    let response = response.ok_or_else(|| anyhow!("WebFetch did not receive a response"))?;
    if is_redirect(response.status) {
        bail!("WebFetch exceeded the safe redirect limit");
    }
    if header(&response, "x-proxy-error") == Some("blocked-by-allowlist") {
        let domain = current.host_str().unwrap_or_default().to_owned();
        // 与 TS JSON.stringify 的键顺序一致（serde_json 会排序，这里显式拼接）。
        let message = format!("Access to {domain} is blocked by the network egress proxy.");
        bail!(
            "{{\"error_type\":\"EGRESS_BLOCKED\",\"domain\":{},\"message\":{}}}",
            Value::from(domain),
            Value::from(message)
        );
    }
    if !(200..300).contains(&response.status) {
        return Ok(Fetched::Terminal(http_error(
            original, &current, &redirects, &response,
        )));
    }
    let content_type = header(&response, "content-type")
        .unwrap_or_default()
        .to_owned();
    let content = rules::extract_readable(&response.body, &content_type).map_err(|m| anyhow!(m))?;
    let (artifact_path, artifact_uri) = match target {
        Some(target) => target.write(&content, &content_type).await?,
        None => (None, None),
    };
    Ok(Fetched::Content(Cached {
        bytes: response.body.len(),
        content,
        content_type,
        final_url: current.to_string(),
        redirects,
        status: response.status,
        status_text: response.status_text,
        artifact_path,
        artifact_uri,
    }))
}

fn redirect_output(
    original: &str,
    from: &Url,
    to: &str,
    redirects: &[Value],
    reply: &Response,
    prompt: &str,
) -> Value {
    let status_text = status_text(reply.status, &reply.status_text);
    let result = [
        "REDIRECT DETECTED: The URL redirects to a different host.".to_owned(),
        String::new(),
        format!("Original URL: {from}"),
        format!("Redirect URL: {to}"),
        format!("Status: {} {status_text}", reply.status),
        String::new(),
        "To complete your request, I need to fetch content from the redirected URL. Please use WebFetch again with these parameters:".to_owned(),
        format!("- url: \"{to}\""),
        format!("- prompt: \"{prompt}\""),
    ]
    .join("\n");
    json!({
        "url": original, "finalUrl": from.to_string(), "status": reply.status,
        "statusText": status_text, "contentType": "text/plain", "bytes": result.len(),
        "result": result, "cacheHit": false, "redirects": redirects, "truncated": false,
    })
}

fn http_error(original: &str, current: &Url, redirects: &[Value], reply: &Response) -> Value {
    let status_text = status_text(reply.status, &reply.status_text);
    let retry_after = header(reply, "retry-after")
        .filter(|v| (1..=6).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()))
        .map(|v| format!("\nRetry-After: {v}"))
        .unwrap_or_default();
    let result = format!(
        "The server returned HTTP {} {status_text}.{retry_after}\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch.",
        reply.status
    );
    json!({
        "url": original, "finalUrl": current.to_string(), "status": reply.status,
        "statusText": status_text, "contentType": "text/plain", "bytes": 0,
        "result": result, "cacheHit": false, "redirects": redirects, "truncated": false,
    })
}

fn status_text(status: u16, text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        return trimmed.to_owned();
    }
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Unknown Status")
        .to_owned()
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// 进程级缓存（TS 模块级 Map）：原始 url 为 key，15 分钟过期，按访问顺序淘汰到 50 MiB 以内。
static CACHE: Mutex<Vec<(String, Instant, Cached)>> = Mutex::new(Vec::new());

fn cache_get(key: &str) -> Option<Cached> {
    let mut cache = CACHE.lock().ok()?;
    let index = cache.iter().position(|(k, _, _)| k == key)?;
    let entry = cache.remove(index);
    if entry.1 <= Instant::now() {
        return None;
    }
    let value = entry.2.clone();
    cache.push(entry);
    Some(value)
}

fn cache_put(key: &str, value: Cached) {
    let size = value.content.len();
    if size > CACHE_MAX_BYTES {
        return;
    }
    let Ok(mut cache) = CACHE.lock() else {
        return;
    };
    let now = Instant::now();
    cache.retain(|(k, expires, _)| k != key && *expires > now);
    cache.push((key.to_owned(), now + CACHE_TTL, value));
    let mut total: usize = cache.iter().map(|(_, _, v)| v.content.len()).sum();
    while total > CACHE_MAX_BYTES && !cache.is_empty() {
        total -= cache.remove(0).2.content.len();
    }
}

#[cfg(test)]
pub(crate) fn clear_cache() {
    CACHE.lock().unwrap().clear();
}

#[cfg(test)]
#[path = "web_fetch_tests.rs"]
mod tests;
