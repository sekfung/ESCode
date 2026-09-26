//! MCP OAuth client_credentials（docs/specs/rust-mcp-oauth.md「client_credentials」）：对齐 SDK 2.0.0
//! `ClientCredentialsProvider` 与 `auth()` 的非交互分支。token 只在连接内存中，不写共享凭据。
use super::mcp_oauth_flow::OAuth;
use super::mcp_oauth_http as http;
use crate::domain::mcp_oauth as rules;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

/// 运行期 token 来源：authorization_code 走共享凭据与刷新锁；client_credentials 在内存中按 401 重新取。
#[derive(Clone)]
pub(super) enum Auth {
    Code(Arc<OAuth>),
    Credentials(Arc<ClientCredentials>),
}
impl Auth {
    pub fn name(&self) -> &str {
        match self {
            Self::Code(oauth) => &oauth.name,
            Self::Credentials(credentials) => &credentials.name,
        }
    }
    pub async fn token(&self) -> Result<Option<String>> {
        match self {
            Self::Code(oauth) => oauth.token().await,
            Self::Credentials(credentials) => Ok(credentials.token().await),
        }
    }
    /// `challenge` 为 401 响应的 `WWW-Authenticate`。
    pub async fn on_unauthorized(&self, challenge: &str) -> Result<()> {
        match self {
            Self::Code(oauth) => oauth.on_unauthorized().await,
            Self::Credentials(credentials) => credentials.authorize(challenge).await,
        }
    }
}

pub(super) struct ClientCredentials {
    name: String,
    url: String,
    client: Value,
    scope: Option<String>,
    tokens: Mutex<Option<Value>>,
}
impl ClientCredentials {
    /// TS `createOAuthClientProvider`：http/sse、非官方鉴权且 `oauth.type=client_credentials`。
    pub fn from_config(name: &str, raw: &Value) -> Option<Self> {
        let oauth = &raw["oauth"];
        if oauth["type"] != "client_credentials" {
            return None;
        }
        Some(Self {
            name: name.into(),
            url: raw["url"].as_str()?.into(),
            client: json!({"client_id": oauth["clientId"].as_str()?, "client_secret": oauth["clientSecret"].as_str()?}),
            scope: oauth["scope"].as_str().map(str::to_owned),
            tokens: Mutex::new(None),
        })
    }
    async fn token(&self) -> Option<String> {
        let tokens = self.tokens.lock().await;
        tokens.as_ref()?["access_token"].as_str().map(str::to_owned)
    }
    /// SDK `auth()` 非交互分支：全量 discovery → resource → determineScope → client_credentials grant。
    async fn authorize(&self, challenge: &str) -> Result<()> {
        let mut tokens = self.tokens.lock().await;
        let challenge = rules::parse_challenge(challenge);
        let discovery = http::server_info(&self.url, challenge.resource_metadata_url.as_deref()).await?;
        let server = discovery["authorizationServerUrl"].as_str().unwrap_or_default().to_owned();
        let metadata = discovery["authorizationServerMetadata"].clone();
        let issuer = metadata["issuer"].as_str().unwrap_or(&server).to_owned();
        let resource = rules::select_resource(&self.url, &discovery["resourceMetadata"]).map_err(|e| anyhow!(e))?;
        let scope = rules::determine_scope(
            challenge.scope.as_deref(),
            &discovery["resourceMetadata"],
            &metadata,
            self.scope.as_deref(),
            &["client_credentials"],
        )
        .or_else(|| self.scope.clone());
        let mut params = vec![("grant_type".to_owned(), "client_credentials".to_owned())];
        if let Some(scope) = scope {
            params.push(("scope".into(), scope));
        }
        let mut issued = http::token_request(&server, &metadata, params, &self.client, resource.as_deref()).await?;
        issued["issuer"] = issuer.into();
        *tokens = Some(issued);
        Ok(())
    }
}
