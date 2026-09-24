//! 代理解析：逐位对应 TS `adapters/src/network/http-config.ts` 的
//! `resolveProxyForRequest` / `resolveWebFetchProxyForRequest`（纯逻辑，无 IO）。
//! 已知差异：Host 侧的环境捕获过滤（`shouldCaptureZCodeToolEnvPassthroughKey`）未移植——
//! Rust 不产出该环境变量，只消费已捕获的键值；带非法键名的项同样被忽略。
use serde_json::Value;

pub const HTTP_PROXY_ENV_KEY: &str = "ZCODE_HTTP_PROXY";
pub const NO_PROXY_ENV_KEY: &str = "ZCODE_NO_PROXY";
pub const TOOL_ENV_PASSTHROUGH_KEY: &str = "ZCODE_TOOL_ENV_PASSTHROUGH_JSON";

/// 捕获的用户代理键，按 TS 顺序取第一个可用值。
const CAPTURED_PROXY_KEYS: [&str; 6] = [
    "https_proxy",
    "HTTPS_PROXY",
    "http_proxy",
    "HTTP_PROXY",
    "all_proxy",
    "ALL_PROXY",
];

#[derive(Default, Clone, Debug)]
pub struct ProxyOptions {
    pub http_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyResolution {
    pub no_proxy_matched: bool,
    pub proxy_source: Option<String>,
    pub proxy_url: Option<String>,
}

impl ProxyResolution {
    fn none() -> Self {
        Self {
            no_proxy_matched: false,
            proxy_source: None,
            proxy_url: None,
        }
    }
    fn bypass() -> Self {
        Self {
            no_proxy_matched: true,
            proxy_source: None,
            proxy_url: None,
        }
    }
    fn via(source: String, proxy_url: String) -> Self {
        Self {
            no_proxy_matched: false,
            proxy_source: Some(source),
            proxy_url: Some(proxy_url),
        }
    }
}

pub fn resolve_proxy_for_request(url: &str, options: &ProxyOptions) -> ProxyResolution {
    resolve(url, options, false)
}

pub fn resolve_webfetch_proxy_for_request(url: &str, options: &ProxyOptions) -> ProxyResolution {
    resolve(url, options, true)
}

fn resolve(url: &str, options: &ProxyOptions, captured_fallback: bool) -> ProxyResolution {
    let Ok(parsed) = url::Url::parse(url) else {
        return ProxyResolution::none();
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return ProxyResolution::none();
    }
    let env = |key: &str| {
        options
            .env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    if should_bypass_proxy(
        &parsed,
        normalize_path_like(options.no_proxy.as_deref())
            .or_else(|| normalize_path_like(env(NO_PROXY_ENV_KEY).as_deref())),
    ) {
        return ProxyResolution::bypass();
    }
    if let Some(proxy) = normalize_proxy_url(options.http_proxy.as_deref()) {
        return ProxyResolution::via("network.httpProxy".into(), proxy);
    }
    if let Some(proxy) = normalize_proxy_url(env(HTTP_PROXY_ENV_KEY).as_deref()) {
        return ProxyResolution::via(format!("env:{HTTP_PROXY_ENV_KEY}"), proxy);
    }
    if !captured_fallback {
        return ProxyResolution::none();
    }
    let captured = captured_env(env(TOOL_ENV_PASSTHROUGH_KEY).as_deref());
    let captured_value = |key: &str| captured.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    if should_bypass_proxy(
        &parsed,
        normalize_path_like(captured_value("no_proxy").as_deref())
            .or_else(|| normalize_path_like(captured_value("NO_PROXY").as_deref())),
    ) {
        return ProxyResolution::bypass();
    }
    for key in CAPTURED_PROXY_KEYS {
        if let Some(proxy) = normalize_proxy_url(captured_value(key).as_deref()) {
            return ProxyResolution::via(format!("env:{TOOL_ENV_PASSTHROUGH_KEY}.{key}"), proxy);
        }
    }
    ProxyResolution::none()
}

/// TS `readZCodeToolEnvPassthroughEnv` 的解析部分：JSON 对象、字符串值、合法键名。
fn captured_env(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return vec![];
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
        return vec![];
    };
    map.into_iter()
        .filter_map(|(key, value)| {
            let valid_key = {
                let mut chars = key.chars();
                chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            };
            let value = value.as_str()?;
            valid_key.then(|| (key, value.to_owned()))
        })
        .collect()
}

fn normalize_path_like(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

/// 缺少 scheme 时按 http 处理；解析失败视为无值。
fn normalize_proxy_url(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if has_scheme(trimmed) {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    url::Url::parse(&candidate).ok().map(|u| u.to_string())
}

fn has_scheme(value: &str) -> bool {
    let Some(idx) = value.find("://") else {
        return false;
    };
    let scheme = &value[..idx];
    !scheme.is_empty()
        && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

fn should_bypass_proxy(url: &url::Url, no_proxy: Option<String>) -> bool {
    let host = normalize_no_proxy_host(url.host_str().unwrap_or(""));
    let Some(no_proxy) = no_proxy else {
        return false;
    };
    if host.is_empty() {
        return false;
    }
    let port = url
        .port()
        .map(|p| p.to_string())
        .unwrap_or_else(|| if url.scheme() == "https" { "443".into() } else { "80".into() });
    for raw in no_proxy.split(',') {
        let Some((token_host, token_port)) = parse_no_proxy_token(raw) else {
            continue;
        };
        if token_host == "*" {
            return true;
        }
        if token_port.is_some_and(|p| p != port) {
            continue;
        }
        if matches_no_proxy_host(&host, &token_host) {
            return true;
        }
    }
    false
}

fn parse_no_proxy_token(raw: &str) -> Option<(String, Option<String>)> {
    let trimmed = raw.trim().to_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "*" {
        return Some(("*".into(), None));
    }
    if trimmed.contains("://") {
        let parsed = url::Url::parse(&trimmed).ok()?;
        return Some((
            normalize_no_proxy_host(parsed.host_str().unwrap_or("")),
            parsed.port().map(|p| p.to_string()).filter(|p| !p.is_empty()),
        ));
    }
    // `[ipv6]` / `[ipv6]:port`：取括号内主机。TS 在缺少 `]` 时 slice(1, -1)，此处同样以末字符为界。
    if trimmed.starts_with('[') {
        let end = match trimmed.find(']') {
            Some(idx) => idx,
            None => trimmed.len().saturating_sub(1).max(1),
        };
        return Some((normalize_no_proxy_host(&trimmed[1..end]), None));
    }
    let colons: Vec<usize> = trimmed.match_indices(':').map(|(i, _)| i).collect();
    if colons.len() == 1 && colons[0] > 0 {
        let split = colons[0];
        return Some((
            normalize_no_proxy_host(&trimmed[..split]),
            Some(trimmed[split + 1..].to_owned()),
        ));
    }
    Some((normalize_no_proxy_host(&trimmed), None))
}

fn normalize_no_proxy_host(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .trim_end_matches('.')
        .to_lowercase()
}

fn matches_no_proxy_host(host: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    if let Some(suffix) = pattern.strip_prefix('.') {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    host == pattern || host.ends_with(&format!(".{pattern}"))
}
