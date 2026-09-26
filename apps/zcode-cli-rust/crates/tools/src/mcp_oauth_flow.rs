//! MCP OAuth 事务（docs/specs/rust-mcp-oauth.md 第 2 层）：运行期 token 与锁内刷新（TS `oauth-provider.ts`、
//! `oauth-refresh.ts`），交互授权 leader/follower（TS `oauth-interactive.ts`）。凭据只经共享加密存储交换。
use super::{mcp_oauth_http as http, mcp_oauth_store as store};
use crate::domain::mcp_oauth::{self as rules, CodeConfig, Pair};
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use zcode_cli_host::{
    credential_store::CredentialStore,
    file_lock::{self, LockTimeout, Timing},
};

/// 交互授权事务寿命（TS `MCP_OAUTH_AUTHORIZATION_TRANSACTION_TTL_MS`）。
pub(super) const TRANSACTION_TTL: Duration = Duration::from_secs(300);
const REFRESH_LOCK: Timing = Timing {
    retry_delays_ms: file_lock::DEFAULT_TIMING.retry_delays_ms,
    ownerless_grace_ms: 100,
    max_wait_ms: 45_000,
};
const LEASE_LOCK: Timing = Timing {
    retry_delays_ms: &[25],
    ownerless_grace_ms: 100,
    max_wait_ms: 250,
};

/// 需要用户交互授权（TS `createInteractiveAuthorizationRequiredError`）。
#[derive(Debug, Default, Clone)]
pub(super) struct AuthorizationRequired {
    pub reason: String,
    pub required_scope: Option<String>,
    pub resource_metadata_url: Option<String>,
}
impl std::fmt::Display for AuthorizationRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP OAuth authorization required ({})", self.reason)
    }
}
impl std::error::Error for AuthorizationRequired {}

/// 临时刷新失败（TS `createTemporaryRefreshFailureError`）：不触发交互授权。
#[derive(Debug)]
pub(super) struct TemporaryRefreshFailure(pub String);
impl std::fmt::Display for TemporaryRefreshFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP OAuth token refresh failed temporarily: {}", self.0)
    }
}
impl std::error::Error for TemporaryRefreshFailure {}

/// 单个 MCP server 的 OAuth 上下文。
pub(super) struct OAuth {
    pub store: Arc<CredentialStore>,
    pub name: String,
    pub url: String,
    pub config: CodeConfig,
    pub prefix: String,
}
impl OAuth {
    pub fn new(store: Arc<CredentialStore>, name: &str, url: &str, config: CodeConfig) -> Self {
        let prefix = rules::key_prefix(name, url, &config);
        Self {
            store,
            name: name.into(),
            url: url.into(),
            config,
            prefix,
        }
    }
    fn lock_path(&self, suffix: &str) -> PathBuf {
        let dir = self.store.path().parent().map(PathBuf::from).unwrap_or_default();
        dir.join(format!("{}.{suffix}", rules::sanitize_key_prefix(&self.prefix)))
    }

    /// TS `AuthProvider.token()`：无 token 返回 None；临期且有 refresh token 时 proactive 刷新。
    pub async fn token(&self) -> Result<Option<String>> {
        let Some(pair) = store::load_pair(&self.store, &self.prefix).await? else {
            return Ok(None);
        };
        let Some(access) = pair.access_token().map(str::to_owned) else {
            return Ok(None);
        };
        if !pair.near_expiry(store::now_ms() as f64) || pair.refresh_token().is_none() {
            return Ok(Some(access));
        }
        self.refresh(false).await.map(Some)
    }
    /// TS `AuthProvider.onUnauthorized()`。
    pub async fn on_unauthorized(&self) -> Result<()> {
        let pair = store::load_pair(&self.store, &self.prefix).await?;
        if pair.as_ref().and_then(Pair::refresh_token).is_none() {
            let reason = if pair.and_then(|p| p.tokens).is_some() { "no_refresh_token" } else { "no_credentials" };
            return Err(AuthorizationRequired { reason: reason.into(), ..Default::default() }.into());
        }
        self.refresh(true).await.map(|_| ())
    }

    /// TS `refreshMcpOAuthTokensUnderLock`：跨进程单飞；等锁期间他人已换代则直接复用。
    pub async fn refresh(&self, reactive: bool) -> Result<String> {
        let observed = store::load_pair(&self.store, &self.prefix).await?.and_then(|p| p.generation);
        let guard = match file_lock::acquire_with(&self.lock_path("refresh"), REFRESH_LOCK).await {
            Ok(guard) => guard,
            Err(error) if error.is::<LockTimeout>() => {
                let current = store::load_pair(&self.store, &self.prefix).await?;
                if let Some(current) = current.filter(|p| p.tokens.is_some() && p.generation != observed) {
                    return Ok(current.access_token().unwrap_or_default().to_owned());
                }
                return Err(TemporaryRefreshFailure(error.to_string()).into());
            }
            Err(error) => return Err(error),
        };
        let result = self.refresh_locked(observed, reactive).await;
        guard.release().await;
        result
    }
    async fn refresh_locked(&self, observed: Option<String>, reactive: bool) -> Result<String> {
        let current = store::load_pair(&self.store, &self.prefix).await?;
        let Some(current) = current.filter(|p| p.tokens.is_some()) else {
            return Err(AuthorizationRequired { reason: "no_credentials".into(), ..Default::default() }.into());
        };
        if current.generation != observed {
            return Ok(current.access_token().unwrap_or_default().to_owned());
        }
        let (Some(refresh_token), Some(client)) = (current.refresh_token().map(str::to_owned), current.client.clone()) else {
            return Err(AuthorizationRequired { reason: "no_refresh_token".into(), ..Default::default() }.into());
        };
        let fail_soft = |error: anyhow::Error| -> Result<String> {
            if reactive {
                Err(TemporaryRefreshFailure(error.to_string()).into())
            } else {
                Ok(current.access_token().unwrap_or_default().to_owned())
            }
        };
        let (server, metadata, resource) = match self.resolve_metadata(current.issuer.as_deref()).await {
            Ok(resolved) => resolved,
            Err(error) => return fail_soft(error),
        };
        match http::refresh(&server, &metadata, &client, &refresh_token, resource.as_deref()).await {
            Ok(tokens) => {
                store::publish(&self.store, &self.prefix, &client, &tokens, current.issuer.as_deref(), &format!("refresh:{}", self.prefix)).await?;
                Ok(tokens["access_token"].as_str().unwrap_or_default().to_owned())
            }
            Err(error) => {
                let code = error.downcast_ref::<http::OAuthError>().map(|e| e.code.clone());
                match code.as_deref() {
                    Some("invalid_grant") => {
                        if let Some(raw) = &current.raw {
                            store::invalidate(&self.store, &self.prefix, raw, false).await?;
                        }
                        Err(AuthorizationRequired { reason: "invalid_grant".into(), ..Default::default() }.into())
                    }
                    Some("invalid_client" | "unauthorized_client") => {
                        if self.config.client_id.is_some() {
                            return Err(anyhow!(
                                "MCP server {} OAuth client was rejected by the authorization server (invalid_client). The configured clientId is not usable; fix the MCP oauth configuration.",
                                self.name
                            ));
                        }
                        if let Some(raw) = &current.raw {
                            store::invalidate(&self.store, &self.prefix, raw, true).await?;
                        }
                        Err(AuthorizationRequired { reason: "invalid_client".into(), ..Default::default() }.into())
                    }
                    _ => fail_soft(error),
                }
            }
        }
    }
    /// TS `resolveAsMetadata`：共享 discovery 缓存（按 issuer 校验）或重新发现；resource 按 RFC 8707 带上。
    async fn resolve_metadata(&self, issuer: Option<&str>) -> Result<(String, Value, Option<String>)> {
        let state = match store::load_discovery(&self.store, &self.prefix, issuer).await? {
            Some(state) => state,
            None => {
                let state = http::server_info(&self.url, None).await?;
                store::save_discovery(&self.store, &self.prefix, &state).await?;
                state
            }
        };
        let resource = rules::select_resource(&self.url, &state["resourceMetadata"]).map_err(|e| anyhow!(e))?;
        Ok((
            state["authorizationServerUrl"].as_str().unwrap_or_default().to_owned(),
            state["authorizationServerMetadata"].clone(),
            resource,
        ))
    }
}

/// 交互授权结果（TS `McpInteractiveAuthorizationOutcome`）。
pub(super) enum Outcome {
    Authorized,
    AlreadyAuthorized,
    Pending,
    Failed(anyhow::Error),
}

/// 授权 URL 出现时通知状态（TS `onAuthorizationRequired`）。
pub(super) type OnUrl = Arc<dyn Fn(String) + Send + Sync>;

impl OAuth {
    /// TS `runMcpInteractiveAuthorization`。
    pub async fn authorize(&self, trigger: &AuthorizationRequired, on_url: OnUrl, cancel: &CancellationToken) -> Outcome {
        match self.authorize_inner(trigger, on_url, cancel).await {
            Ok(outcome) => outcome,
            Err(error) => Outcome::Failed(error),
        }
    }
    async fn authorize_inner(&self, trigger: &AuthorizationRequired, on_url: OnUrl, cancel: &CancellationToken) -> Result<Outcome> {
        let baseline = store::load_canonical(&self.store, &self.prefix).await?.and_then(|p| p.generation);
        let lease = match file_lock::acquire_with(&self.lock_path("authz"), LEASE_LOCK).await {
            Ok(lease) => lease,
            Err(error) if error.is::<LockTimeout>() => return self.follow(baseline, on_url, cancel).await,
            Err(error) => return Err(error),
        };
        let attempt = hex(&zcode_cli_host::credential_cipher::random_bytes(16));
        let result = async {
            if self.newer(&baseline).await? {
                return Ok(Outcome::AlreadyAuthorized);
            }
            Ok(self.lead(trigger, &attempt, baseline.as_deref(), on_url, cancel).await)
        }
        .await;
        let _ = store::delete_pending_if_owned(&self.store, &self.prefix, &attempt).await;
        lease.release().await;
        result
    }
    async fn newer(&self, baseline: &Option<String>) -> Result<bool> {
        Ok(store::load_canonical(&self.store, &self.prefix)
            .await?
            .is_some_and(|p| p.tokens.is_some() && p.generation != *baseline))
    }
    /// TS `followAuthorization`：轮询 canonical 换代与共享 pending URL。
    async fn follow(&self, baseline: Option<String>, on_url: OnUrl, cancel: &CancellationToken) -> Result<Outcome> {
        let deadline = tokio::time::Instant::now() + TRANSACTION_TTL;
        let mut projected: Option<String> = None;
        while tokio::time::Instant::now() < deadline && !cancel.is_cancelled() {
            if self.newer(&baseline).await? {
                return Ok(Outcome::AlreadyAuthorized);
            }
            if let Some(url) = store::load_pending(&self.store, &self.prefix).await?
                && projected.as_deref() != Some(url.as_str())
            {
                projected = Some(url.clone());
                on_url(url);
            }
            tokio::select! {_ = cancel.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(500)) => {}}
        }
        Ok(Outcome::Pending)
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[path = "mcp_oauth_lead.rs"]
mod lead;
