//! MCP OAuth 的网络部分（docs/specs/rust-mcp-oauth.md 第 2 层），按 `@modelcontextprotocol/client@2.0.0`
//! 的 `discoverOAuthServerInfo`、`registerClient`、`startAuthorization`、`executeTokenRequest` 移植。
use crate::domain::mcp_oauth as rules;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use url::Url;

/// 授权服务器返回的 OAuth 错误（SDK `OAuthError`）：调用方按 code 分类。
#[derive(Debug)]
pub(super) struct OAuthError {
    pub code: String,
    pub description: String,
}
impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.description)
    }
}
impl std::error::Error for OAuthError {}

pub(super) fn client() -> reqwest_mcp::Client {
    static CLIENT: std::sync::OnceLock<reqwest_mcp::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest_mcp::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("MCP OAuth HTTP client")
        })
        .clone()
}

async fn get(url: &str, accept_json: bool) -> Result<reqwest_mcp::Response> {
    let mut request = client()
        .get(url)
        .header("MCP-Protocol-Version", rules::DISCOVERY_PROTOCOL_VERSION);
    if accept_json {
        request = request.header("Accept", "application/json");
    }
    Ok(request.send().await?)
}
fn fallback_status(status: u16) -> bool {
    (400..500).contains(&status) || status == 502
}

/// SDK `discoverOAuthProtectedResourceMetadata`（RFC 9728）：404 与不支持均为错误。
pub(super) async fn protected_resource(server_url: &str, metadata_url: Option<&str>) -> Result<Value> {
    let response = match metadata_url {
        Some(url) => get(url, false).await?,
        None => {
            let (first, fallback) = rules::protected_resource_urls(server_url).context("Invalid MCP server URL")?;
            let response = get(&first, false).await?;
            match fallback {
                Some(fallback) if fallback_status(response.status().as_u16()) => get(&fallback, false).await?,
                _ => response,
            }
        }
    };
    let status = response.status().as_u16();
    if status == 404 {
        bail!("Resource server does not implement OAuth 2.0 Protected Resource Metadata.");
    }
    if !response.status().is_success() {
        bail!("HTTP {status} trying to load well-known OAuth protected resource metadata.");
    }
    let value: Value = response.json().await?;
    anyhow::ensure!(value["resource"].is_string(), "Invalid protected resource metadata");
    Ok(value)
}

/// SDK `discoverAuthorizationServerMetadata`：按 RFC 8414 → OIDC 顺序；4xx/502 继续，其余错误中止；校验 issuer 回显。
pub(super) async fn authorization_server_metadata(authorization_server: &str) -> Result<Option<Value>> {
    for (url, oidc) in rules::discovery_urls(authorization_server) {
        let response = get(&url, true).await?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            if fallback_status(status) {
                continue;
            }
            bail!(
                "HTTP {status} trying to load {} metadata from {url}",
                if oidc { "OpenID provider" } else { "OAuth" }
            );
        }
        let metadata: Value = response.json().await?;
        for field in ["issuer", "authorization_endpoint", "token_endpoint"] {
            anyhow::ensure!(metadata[field].is_string(), "Invalid authorization server metadata: {field}");
        }
        let issuer = metadata["issuer"].as_str().unwrap();
        let exact = issuer == authorization_server
            || authorization_server.strip_suffix('/').is_some_and(|e| e == issuer);
        anyhow::ensure!(
            exact,
            "Authorization server metadata issuer mismatch: expected {authorization_server}, got {issuer}"
        );
        return Ok(Some(metadata));
    }
    Ok(None)
}

/// SDK `discoverOAuthServerInfo` 的结果形态（即共享缓存中的 `OAuthDiscoveryState`）。
pub(super) async fn server_info(server_url: &str, metadata_url: Option<&str>) -> Result<Value> {
    // 非网络错误（404、解析失败）静默回落到以 server origin 为授权服务器。
    let resource = match protected_resource(server_url, metadata_url).await {
        Ok(resource) => Some(resource),
        Err(error) if error.is::<reqwest_mcp::Error>() => return Err(error),
        Err(_) => None,
    };
    let authorization_server = resource
        .as_ref()
        .and_then(|r| r["authorization_servers"][0].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            Url::parse(server_url)
                .and_then(|u| u.join("/"))
                .map(|u| u.to_string())
                .unwrap_or_default()
        });
    let metadata = authorization_server_metadata(&authorization_server).await?;
    let mut state = json!({"authorizationServerUrl": authorization_server});
    if let Some(url) = metadata_url {
        state["resourceMetadataUrl"] = url.into();
    }
    if let Some(resource) = resource {
        state["resourceMetadata"] = resource;
    }
    if let Some(metadata) = metadata {
        state["authorizationServerMetadata"] = metadata;
    }
    Ok(state)
}

/// SDK `registerClient`（RFC 7591）：请求体 = client metadata，`scope` 以本次选定值覆盖。
pub(super) async fn register(authorization_server: &str, metadata: &Value, client_metadata: &Value, scope: Option<&str>) -> Result<Value> {
    let url = match metadata.get("registration_endpoint") {
        Some(Value::String(url)) => url.clone(),
        _ if metadata.is_object() => bail!("Incompatible auth server: does not support dynamic client registration"),
        _ => Url::parse(authorization_server)?.join("/register")?.to_string(),
    };
    let mut body = client_metadata.clone();
    if let Some(scope) = scope {
        body["scope"] = scope.into();
    }
    let response = client().post(url).header("Content-Type", "application/json").body(body.to_string()).send().await?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!("Dynamic client registration rejected: HTTP {status}: {text}");
    }
    let info: Value = response.json().await?;
    anyhow::ensure!(info["client_id"].is_string(), "Invalid client registration response");
    Ok(info)
}

const VERIFIER_CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// SDK `startAuthorization`：PKCE S256（43 字符 verifier，同 `pkce-challenge` 默认），参数顺序同 SDK。
pub(super) fn authorization_url(
    authorization_server: &str,
    metadata: &Value,
    client_id: &str,
    redirect_url: &str,
    state: &str,
    scope: Option<&str>,
    resource: Option<&str>,
) -> Result<(String, String)> {
    let mut url = if metadata.is_object() {
        let supports = |key: &str, value: &str| metadata[key].as_array().is_some_and(|a| a.iter().any(|v| v == value));
        if !supports("response_types_supported", "code") {
            bail!("Incompatible auth server: does not support response type code");
        }
        if metadata["code_challenge_methods_supported"].is_array() && !supports("code_challenge_methods_supported", "S256") {
            bail!("Incompatible auth server: does not support code challenge method S256");
        }
        Url::parse(metadata["authorization_endpoint"].as_str().unwrap_or_default())?
    } else {
        Url::parse(authorization_server)?.join("/authorize")?
    };
    let random = zcode_cli_host::credential_cipher::random_bytes(43);
    let verifier: String = random.iter().map(|b| VERIFIER_CHARSET[*b as usize % VERIFIER_CHARSET.len()] as char).collect();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("code_challenge", &challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("redirect_uri", redirect_url);
        query.append_pair("state", state);
        if let Some(scope) = scope {
            query.append_pair("scope", scope);
            if scope.split(' ').any(|s| s == "offline_access") {
                query.append_pair("prompt", "consent");
            }
        }
        if let Some(resource) = resource {
            query.append_pair("resource", resource);
        }
    }
    Ok((url.to_string(), verifier))
}

/// SDK `assertSecureTokenEndpoint`：非 https 仅允许 loopback。
fn secure_token_endpoint(endpoint: &str) -> Result<Url> {
    let url = Url::parse(endpoint)?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    anyhow::ensure!(url.scheme() == "https" || loopback, "Refusing to send OAuth credentials to insecure token endpoint {url}");
    Ok(url)
}

/// SDK `executeTokenRequest`：form 编码、resource、按 `selectClientAuthMethod` 认证；非 2xx 解析为 OAuth 错误。
pub(super) async fn token_request(
    authorization_server: &str,
    metadata: &Value,
    mut params: Vec<(String, String)>,
    client_info: &Value,
    resource: Option<&str>,
) -> Result<Value> {
    let endpoint = match metadata["token_endpoint"].as_str() {
        Some(endpoint) => endpoint.to_owned(),
        None => Url::parse(authorization_server)?.join("/token")?.to_string(),
    };
    let url = secure_token_endpoint(&endpoint)?;
    if let Some(resource) = resource {
        params.push(("resource".into(), resource.into()));
    }
    let supported: Vec<&str> = metadata["token_endpoint_auth_methods_supported"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let client_id = client_info["client_id"].as_str().unwrap_or_default().to_owned();
    let secret = client_info["client_secret"].as_str().map(str::to_owned);
    let mut request = client()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json");
    match rules::client_auth_method(client_info, &supported) {
        "client_secret_basic" => {
            let secret = secret.context("client_secret_basic authentication requires a client_secret")?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{secret}"));
            request = request.header("Authorization", format!("Basic {encoded}"));
        }
        "client_secret_post" => {
            params.push(("client_id".into(), client_id));
            if let Some(secret) = secret {
                params.push(("client_secret".into(), secret));
            }
        }
        _ => params.push(("client_id".into(), client_id)),
    }
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(&params).finish();
    let response = request.body(body).send().await?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let error = |value: Option<&Value>| match value.and_then(|v| v["error"].as_str()) {
        Some(code) => anyhow!(OAuthError {
            code: code.to_owned(),
            description: value.and_then(|v| v["error_description"].as_str()).unwrap_or_default().to_owned(),
        }),
        None => anyhow!(OAuthError {
            code: "server_error".into(),
            description: format!("HTTP {status}: Invalid OAuth error response. Raw body: {text}"),
        }),
    };
    if !(200..300).contains(&status) {
        return Err(error(parsed.as_ref()));
    }
    match parsed {
        Some(tokens) if tokens["access_token"].is_string() && tokens["token_type"].is_string() => Ok(tokens),
        Some(value) if value.get("error").is_some() => Err(error(Some(&value))),
        _ => Err(anyhow!("Invalid OAuth token response")),
    }
}

/// SDK `refreshAuthorization`：返回体缺 refresh_token 时保留原值。
pub(super) async fn refresh(authorization_server: &str, metadata: &Value, client_info: &Value, refresh_token: &str, resource: Option<&str>) -> Result<Value> {
    let params = vec![("grant_type".into(), "refresh_token".into()), ("refresh_token".into(), refresh_token.into())];
    let mut tokens = json!({"refresh_token": refresh_token});
    let response = token_request(authorization_server, metadata, params, client_info, resource).await?;
    for (key, value) in response.as_object().into_iter().flatten() {
        tokens[key] = value.clone();
    }
    Ok(tokens)
}
