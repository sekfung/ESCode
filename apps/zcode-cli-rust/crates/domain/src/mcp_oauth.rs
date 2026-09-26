//! MCP OAuth 纯规则（docs/specs/rust-mcp-oauth.md 第 2 层），逐条对齐 TS `adapters/src/mcp/oauth*.ts`
//! 与 `@modelcontextprotocol/client@2.0.0` 的 auth 辅助函数。语料由
//! `scripts/generate-zcode-cli-rust-mcp-oauth-corpus.mjs` 从 TS oracle 生成。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

pub const CANONICAL_KEY: &str = "authorization_credentials";
pub const LEGACY_CLIENT_KEY: &str = "client_information";
pub const LEGACY_TOKENS_KEY: &str = "tokens";
pub const DISCOVERY_STATE_KEY: &str = "discovery_state";
pub const DISCOVERY_FETCHED_AT_KEY: &str = "discovery_state_fetched_at";
pub const PENDING_KEY: &str = "pending_authorization";
/// SDK `LATEST_PROTOCOL_VERSION`，discovery 请求头 `MCP-Protocol-Version`。
pub const DISCOVERY_PROTOCOL_VERSION: &str = "2025-11-25";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// authorization_code 配置（TS `McpOAuthConfig` 的 authorization_code 分支）。
#[derive(Clone, Default, Debug, PartialEq)]
pub struct CodeConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub client_name: Option<String>,
    pub scope: Option<String>,
    pub redirect_path: Option<String>,
}

/// TS `resolveAuthorizationCodeOAuthConfig`：http/sse 且非官方鉴权、非 client_credentials、无 Authorization 头
/// 时启用（未写 oauth 字段的 server 也启用，由 401/403 触发）。
pub fn code_config(server: &Value, transport: &str) -> Option<CodeConfig> {
    if transport == "stdio" {
        return None;
    }
    let official = matches!(transport, "http" | "stdio")
        && server["auth"]["type"] == "zcode_official"
        && server["auth"]["provider"] == "jwt_token"
        && !server["official"].is_null();
    if official {
        return None;
    }
    let text = |key: &str| server["oauth"][key].as_str().map(str::to_owned);
    match server["oauth"]["type"].as_str() {
        Some("authorization_code") => {
            return Some(CodeConfig {
                client_id: text("clientId"),
                client_secret: text("clientSecret"),
                client_name: text("clientName"),
                scope: text("scope"),
                redirect_path: text("redirectPath"),
            });
        }
        Some("client_credentials") => return None,
        _ => {}
    }
    let has_authorization = server["headers"]
        .as_object()
        .is_some_and(|h| h.keys().any(|k| k.eq_ignore_ascii_case("authorization")));
    (!has_authorization).then(CodeConfig::default)
}

/// TS `createCredentialKeyPrefix`。
pub fn key_prefix(name: &str, url: &str, config: &CodeConfig) -> String {
    let joined = [
        name,
        url,
        config.client_id.as_deref().unwrap_or(""),
        config.scope.as_deref().unwrap_or(""),
        config.redirect_path.as_deref().unwrap_or(""),
    ]
    .join("\n");
    format!("mcp:oauth:{}", &hex(&Sha256::digest(joined.as_bytes()))[..24])
}
pub fn key(prefix: &str, name: &str) -> String {
    format!("{prefix}:{name}")
}
/// TS `sanitizeKeyPrefix`：lease/refresh 锁文件名只含字母数字与连字符。
pub fn sanitize_key_prefix(prefix: &str) -> String {
    prefix
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

/// JS `encodeURIComponent`。
fn encode_uri_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
/// TS `normalizeCallbackPath`。
pub fn callback_path(redirect_path: Option<&str>, server_name: &str) -> String {
    match redirect_path.filter(|p| !p.is_empty()) {
        Some(path) if path.starts_with('/') => path.to_owned(),
        Some(path) => format!("/{path}"),
        None => format!("/oauth/callback/mcp/{}", encode_uri_component(server_name)),
    }
}

/// SDK `computeScopeUnion`：去重、保持首次出现顺序；全空返回 None。
pub fn scope_union(scopes: &[Option<&str>]) -> Option<String> {
    let mut seen: Vec<&str> = vec![];
    for scope in scopes.iter().flatten() {
        for token in scope.split_whitespace() {
            if !seen.contains(&token) {
                seen.push(token);
            }
        }
    }
    (!seen.is_empty()).then(|| seen.join(" "))
}

/// SDK `determineScope`（未导出，按源码移植）。
pub fn determine_scope(
    requested: Option<&str>,
    resource_metadata: &Value,
    as_metadata: &Value,
    client_scope: Option<&str>,
    grant_types: &[&str],
) -> Option<String> {
    let resource_scopes = resource_metadata["scopes_supported"].as_array().map(|s| {
        s.iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ")
    });
    let mut effective = requested
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or(resource_scopes.filter(|s| !s.is_empty()))
        .or(client_scope.filter(|s| !s.is_empty()).map(str::to_owned))?;
    let offline = as_metadata["scopes_supported"]
        .as_array()
        .is_some_and(|s| s.iter().any(|v| v == "offline_access"));
    if offline
        && !effective.split(' ').any(|s| s == "offline_access")
        && grant_types.contains(&"refresh_token")
    {
        effective.push_str(" offline_access");
    }
    Some(effective)
}

#[derive(Default, Debug, PartialEq)]
pub struct Challenge {
    pub resource_metadata_url: Option<String>,
    pub scope: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}
/// SDK `extractWWWAuthenticateParams`：只解析 Bearer；字段取 `name="v"` 或 `name=v`。
pub fn parse_challenge(header: &str) -> Challenge {
    let mut parts = header.split(' ');
    let (Some(kind), Some(rest)) = (parts.next(), parts.next()) else {
        return Challenge::default();
    };
    if !kind.eq_ignore_ascii_case("bearer") || rest.is_empty() {
        return Challenge::default();
    }
    let field = |name: &str| -> Option<String> {
        let needle = format!("{name}=");
        let start = header.find(&needle)? + needle.len();
        let tail = &header[start..];
        let value = match tail.strip_prefix('"') {
            Some(quoted) => &quoted[..quoted.find('"')?],
            None => tail.split([' ', '\t', '\n', ',']).next()?,
        };
        (!value.is_empty()).then(|| value.to_owned())
    };
    Challenge {
        resource_metadata_url: field("resource_metadata")
            .and_then(|u| Url::parse(&u).ok())
            .map(|u| u.to_string()),
        scope: field("scope"),
        error: field("error"),
        error_description: field("error_description"),
    }
}

/// SDK `buildWellKnownPath` + `discoverMetadataWithFallback`：先按路径插入，再（非根路径、4xx/502/无响应时）回落到根。
pub fn protected_resource_urls(server_url: &str) -> Option<(String, Option<String>)> {
    let issuer = Url::parse(server_url).ok()?;
    let mut path = issuer.path().to_owned();
    if path.ends_with('/') {
        path.pop();
    }
    let mut first = issuer.join(&format!("/.well-known/oauth-protected-resource{path}")).ok()?;
    first.set_query(issuer.query());
    let fallback = (issuer.path() != "/")
        .then(|| issuer.join("/.well-known/oauth-protected-resource").ok())
        .flatten()
        .map(|u| u.to_string());
    Some((first.to_string(), fallback))
}

/// SDK `buildDiscoveryUrls`：`(url, is_oidc)`，按优先顺序。
pub fn discovery_urls(authorization_server: &str) -> Vec<(String, bool)> {
    let Ok(url) = Url::parse(authorization_server) else {
        return vec![];
    };
    let origin = url.origin().ascii_serialization();
    if url.path() == "/" {
        return vec![
            (format!("{origin}/.well-known/oauth-authorization-server"), false),
            (format!("{origin}/.well-known/openid-configuration"), true),
        ];
    }
    let path = url.path().trim_end_matches('/');
    vec![
        (format!("{origin}/.well-known/oauth-authorization-server{path}"), false),
        (format!("{origin}/.well-known/openid-configuration{path}"), true),
        (format!("{origin}{path}/.well-known/openid-configuration"), true),
    ]
}

/// SDK `resourceUrlFromServerUrl`：去掉 fragment。
pub fn resource_url(server_url: &str) -> Option<String> {
    let mut url = Url::parse(server_url).ok()?;
    url.set_fragment(None);
    Some(url.to_string())
}
/// SDK `checkResourceAllowed`：同 origin 且请求路径以配置路径（补 `/`）为前缀。
pub fn resource_allowed(requested: &str, configured: &str) -> bool {
    let (Ok(requested), Ok(configured)) = (Url::parse(requested), Url::parse(configured)) else {
        return false;
    };
    if requested.origin() != configured.origin()
        || requested.path().len() < configured.path().len()
    {
        return false;
    }
    let slash = |p: &str| if p.ends_with('/') { p.to_owned() } else { format!("{p}/") };
    slash(requested.path()).starts_with(&slash(configured.path()))
}
/// SDK `selectResourceURL`（无 provider 校验）：无 resource metadata 时不带 resource；不匹配报错。
pub fn select_resource(server_url: &str, resource_metadata: &Value) -> Result<Option<String>, String> {
    let Some(configured) = resource_metadata["resource"].as_str() else {
        return Ok(None);
    };
    let requested = resource_url(server_url).ok_or("Invalid MCP server URL")?;
    if !resource_allowed(&requested, configured) {
        return Err(format!(
            "Protected resource {configured} does not match expected {requested} (or origin)"
        ));
    }
    Ok(Url::parse(configured).ok().map(|u| u.to_string()))
}

/// SDK `selectClientAuthMethod`。
pub fn client_auth_method(client: &Value, supported: &[&str]) -> &'static str {
    let known = ["client_secret_basic", "client_secret_post", "none"];
    let has_secret = !client["client_secret"].is_null() && client.get("client_secret").is_some();
    if let Some(method) = client["token_endpoint_auth_method"].as_str()
        && let Some(known) = known.iter().find(|m| **m == method)
        && (supported.is_empty() || supported.contains(&method))
    {
        return known;
    }
    if supported.is_empty() {
        return if has_secret { "client_secret_basic" } else { "none" };
    }
    if has_secret && supported.contains(&"client_secret_basic") {
        return "client_secret_basic";
    }
    if has_secret && supported.contains(&"client_secret_post") {
        return "client_secret_post";
    }
    if supported.contains(&"none") {
        return "none";
    }
    if has_secret { "client_secret_post" } else { "none" }
}

/// SDK `deriveApplicationType`。
fn application_type(redirect_uris: &[&str]) -> &'static str {
    for raw in redirect_uris {
        let Ok(url) = Url::parse(raw) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            return "native";
        }
        if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1")) {
            return "native";
        }
    }
    "web"
}
/// TS `InteractiveAuthorizationProvider.clientMetadata` 经 SDK `resolveClientMetadata` 补默认值后的 DCR 请求体。
pub fn client_metadata(config: &CodeConfig, server_name: &str, redirect_url: &str, scope: Option<&str>) -> Value {
    let mut metadata = json!({
        "client_name": config.client_name.clone().unwrap_or_else(|| format!("ZCode {server_name}")),
        "grant_types": ["authorization_code", "refresh_token"],
        "redirect_uris": [redirect_url],
        "response_types": ["code"],
    });
    if config.client_secret.is_some() {
        metadata["token_endpoint_auth_method"] = "client_secret_basic".into();
    }
    if let Some(scope) = scope.or(config.scope.as_deref()) {
        metadata["scope"] = scope.into();
    }
    metadata["application_type"] = application_type(&[redirect_url]).into();
    metadata
}

/// SDK `issuersMatch`：容忍单个结尾 `/` 差异。
pub fn issuers_match(a: &str, b: &str) -> bool {
    a == b
        || a.strip_suffix('/').is_some_and(|x| x == b)
        || b.strip_suffix('/').is_some_and(|x| x == a)
}

pub use super::mcp_oauth_pair::{Pair, derive_pair};

#[cfg(test)]
#[path = "mcp_oauth_tests.rs"]
mod tests;
