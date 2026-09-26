//! 官方 MCP 鉴权纯规则（docs/specs/rust-mcp-official-auth.md 第 1 期），对齐 TS
//! `packages/shared/src/official-mcp-auth.ts`、`zcodeEndpoint.ts` 与 adapters `official-auth.ts`。
use serde_json::Value;
use url::Url;

/// stdio 身份载荷在出站消息 `params._meta` 上的键（跨语言协议常量）。
pub const META_KEY: &str = "com.zcode/official-mcp-auth";
pub const DEV_TRUSTED_ORIGINS_ENV: &str = "ZCODE_OFFICIAL_MCP_DEV_TRUSTED_ORIGINS";
pub const DEFAULT_ZCODE_ORIGIN: &str = "https://zcode.z.ai";
/// 静态 headers 中禁止出现的保留头（小写）。
pub const RESERVED_HEADERS: [&str; 8] = [
    "authorization",
    "x-bigmodel-authorization",
    "bigmodel-target-type",
    "bigmodel-organization",
    "bigmodel-project",
    "x-coding-plan-api-key",
    "mcp-session-id",
    "mcp-protocol-version",
];
/// 合并身份头时一律剥离的关联头（不外发，只从响应读服务端 request id）。
pub const CORRELATION_HEADERS: [&str; 2] = ["x-request-id", "x-trace-id"];

pub fn is_reserved(name: &str) -> bool {
    RESERVED_HEADERS.contains(&name.trim().to_ascii_lowercase().as_str())
}

/// TS `findOfficialMcpReservedHeaders`：命中的保留头，小写、去重、排序。
pub fn reserved_headers<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut hits: Vec<String> = names
        .into_iter()
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| RESERVED_HEADERS.contains(&n.as_str()))
        .collect();
    hits.sort();
    hits.dedup();
    hits
}

/// TS `parseZCodeOfficialAuth`：未声明为 Ok(false)；声明但不合法一律报错（不宽容降级）。
pub fn parse_auth(value: Option<&Value>, mcp_key: &str) -> Result<bool, String> {
    let Some(value) = value else { return Ok(false) };
    let Some(object) = value.as_object() else {
        return Err(format!("MCP server {mcp_key}: auth must be an object"));
    };
    let text = |key: &str| match object.get(key) {
        Some(Value::String(s)) => s.clone(),
        None => "undefined".into(),
        Some(other) => other.to_string(),
    };
    if object.get("type").and_then(Value::as_str) != Some("zcode_official") {
        return Err(format!("MCP server {mcp_key}: unsupported auth type: {}", text("type")));
    }
    if object.get("provider").and_then(Value::as_str) != Some("jwt_token") {
        return Err(format!("MCP server {mcp_key}: unsupported auth provider: {}", text("provider")));
    }
    Ok(true)
}

/// TS `resolveRuntimeZCodeEndpointOrigin`：`ZCODE_BASE_URL`，其次 `ZCODE_ENDPOINT_ORIGIN`（均 trim 后非空），缺省生产 origin。
pub fn zcode_api_origin(base_url: Option<&str>, endpoint_origin: Option<&str>) -> Result<String, String> {
    let pick = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    let Some(origin) = pick(base_url).or_else(|| pick(endpoint_origin)) else {
        return Ok(DEFAULT_ZCODE_ORIGIN.into());
    };
    let parsed = Url::parse(&origin).map_err(|_| "Invalid URL".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("ZCode endpoint origin must use http or https".into());
    }
    Ok(parsed.origin().ascii_serialization())
}

fn without_userinfo(candidate: &str) -> Option<Url> {
    let url = Url::parse(candidate).ok()?;
    (url.username().is_empty() && url.password().is_none()).then_some(url)
}
fn https_origin(candidate: &str) -> Option<String> {
    let url = without_userinfo(candidate)?;
    (url.scheme() == "https").then(|| url.origin().ascii_serialization())
}
fn loopback_origin(candidate: &str) -> Option<String> {
    let url = without_userinfo(candidate)?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    (url.scheme() == "http" && loopback).then(|| url.origin().ascii_serialization())
}

/// TS `isOfficialMcpOriginTrusted`：返回 (trusted, detail)。dev 开关只放开列出的 http loopback origin。
pub fn origin_trusted(origin: &str, dev_trusted: Option<&str>, zcode_api_origin: Option<&str>) -> (bool, &'static str) {
    let origin = origin.trim();
    if origin.is_empty() {
        return (false, "invalid_input");
    }
    if let Some(loopback) = loopback_origin(origin) {
        let listed = dev_trusted.map(str::trim).unwrap_or_default();
        if listed
            .split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .any(|c| loopback_origin(c).as_deref() == Some(loopback.as_str()))
        {
            return (true, "ok");
        }
    }
    let Some(expected) = zcode_api_origin.and_then(https_origin) else {
        return (false, "zcode_origin_unresolved");
    };
    if https_origin(origin).as_deref() != Some(expected.as_str()) {
        return (false, "origin_mismatch");
    }
    (true, "ok")
}

/// TS `mergeOfficialAuthHeaders`：身份头覆盖同名头；其余保留头（除协议头）与关联头丢弃。
pub fn merge_headers(incoming: &[(String, String)], auth: &[(String, String)]) -> Vec<(String, String)> {
    let auth_names: Vec<String> = auth.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
    let mut merged: Vec<(String, String)> = incoming
        .iter()
        .filter(|(name, _)| {
            let lower = name.to_ascii_lowercase();
            !auth_names.contains(&lower)
                && !CORRELATION_HEADERS.contains(&lower.as_str())
                && (!is_reserved(&lower) || lower == "mcp-session-id" || lower == "mcp-protocol-version")
        })
        .cloned()
        .collect();
    merged.extend(auth.iter().cloned());
    merged
}

/// 网络层鉴权失败（TS `createOfficialMcpAuthFetch` 末尾）：401/403/3xx 抛出带分类的错误。
pub fn auth_failure(status: u16) -> Option<&'static str> {
    match status {
        401 => Some("official_auth_rejected"),
        403 => Some("official_auth_forbidden"),
        301 | 302 | 303 | 307 | 308 => Some("official_auth_redirect_blocked"),
        _ => None,
    }
}

/// TS `classifyOfficialMcpResponse`（非 tools/call）：状态码与有界 JSON 体上的诊断分类。
pub fn classify_response(status: u16, content_type: &str, body: Option<&str>) -> Option<&'static str> {
    let ok = (200..300).contains(&status);
    let fallback = if ok { None } else { Some("connection_failed") };
    if status == 429 {
        return Some("rate_limited");
    }
    if status >= 500 {
        return Some("server_internal_error");
    }
    if !content_type.to_ascii_lowercase().contains("json") {
        return fallback;
    }
    let Some(record) = body.filter(|b| !b.is_empty()).and_then(|b| serde_json::from_str::<Value>(b).ok()) else {
        return fallback;
    };
    let Some(record) = record.as_object() else { return fallback };
    match record.get("code").and_then(Value::as_i64) {
        Some(3001) => return Some("server_not_found"),
        Some(1000) => return Some("server_unavailable"),
        _ => {}
    }
    if record.get("jsonrpc").and_then(Value::as_str) == Some("2.0") && record.contains_key("error") {
        return Some(match record["error"].get("code").and_then(Value::as_i64) {
            Some(1006) => "not_authenticated",
            Some(3101) => "coding_plan_required",
            _ => "protocol_error",
        });
    }
    fallback
}

#[cfg(test)]
#[path = "mcp_official_auth_tests.rs"]
mod tests;
