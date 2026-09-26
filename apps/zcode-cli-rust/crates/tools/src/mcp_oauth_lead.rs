//! 交互授权 leader（TS `leadAuthorization` + SDK `authInternal` 两腿）与 127.0.0.1 回调监听
//! （TS `localhost-callback.ts`）。只在事务内存保存 DCR client 与 PKCE verifier，换到 token 才整对发布。
use super::{AuthorizationRequired, OAuth, OnUrl, Outcome, TRANSACTION_TTL, hex, http, store};
use crate::domain::mcp_oauth as rules;
use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const SUCCESS_TEXT: &str = "Authorization successful! You may close this window and return to the CLI.";
const FAILURE_TEXT: &str = "Authorization failed. You may close this window and return to the CLI.";

impl OAuth {
    pub(super) async fn lead(
        &self,
        trigger: &AuthorizationRequired,
        attempt: &str,
        baseline: Option<&str>,
        on_url: OnUrl,
        cancel: &CancellationToken,
    ) -> Outcome {
        let state = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(zcode_cli_host::credential_cipher::random_bytes(24));
        let path = rules::callback_path(self.config.redirect_path.as_deref(), &self.name);
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => return Outcome::Failed(error.into()),
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        let redirect = format!("http://127.0.0.1:{port}{path}");
        let stop = cancel.child_token();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(serve_callback(listener, path, state.clone(), sender, stop.clone()));
        let result = self
            .exchange(trigger, attempt, baseline, &state, &redirect, receiver, on_url, cancel)
            .await;
        stop.cancel();
        let _ = server.await;
        match result {
            Ok(outcome) => outcome,
            Err(error) => match self.newer(&baseline.map(str::to_owned)).await {
                Ok(true) => Outcome::AlreadyAuthorized,
                _ => Outcome::Failed(error),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn exchange(
        &self,
        trigger: &AuthorizationRequired,
        attempt: &str,
        baseline: Option<&str>,
        state: &str,
        redirect: &str,
        callback: tokio::sync::oneshot::Receiver<Result<String>>,
        on_url: OnUrl,
        cancel: &CancellationToken,
    ) -> Result<Outcome> {
        // 403 step-up：config ∪ 当前 token scope ∪ challenge scope（编排层 computeScopeUnion）。
        let requested = match &trigger.required_scope {
            Some(required) => {
                let current = store::load_pair(&self.store, &self.prefix).await?;
                let token_scope = current
                    .as_ref()
                    .and_then(|p| p.tokens.as_ref())
                    .and_then(|t| t["scope"].as_str())
                    .map(str::to_owned);
                rules::scope_union(&[self.config.scope.as_deref(), token_scope.as_deref(), Some(required)])
            }
            None => self.config.scope.clone(),
        };
        let discovery = self.discover(trigger.resource_metadata_url.as_deref()).await?;
        let server = discovery["authorizationServerUrl"].as_str().unwrap_or_default().to_owned();
        let metadata = discovery["authorizationServerMetadata"].clone();
        let issuer = metadata["issuer"].as_str().unwrap_or(&server).to_owned();
        let resource = rules::select_resource(&self.url, &discovery["resourceMetadata"]).map_err(|e| anyhow!(e))?;
        let client_metadata = rules::client_metadata(&self.config, &self.name, redirect, requested.as_deref());
        let scope = rules::determine_scope(
            requested.as_deref(),
            &discovery["resourceMetadata"],
            &metadata,
            requested.as_deref().or(self.config.scope.as_deref()),
            &["authorization_code", "refresh_token"],
        );
        // 静态 clientId 直接使用；否则每次 fresh DCR（旧 client 的 redirect_uris 锁定在旧端口）。
        let mut client = match &self.config.client_id {
            Some(id) => {
                let mut client = json!({"client_id": id});
                if let Some(secret) = &self.config.client_secret {
                    client["client_secret"] = secret.clone().into();
                }
                client
            }
            None => http::register(&server, &metadata, &client_metadata, scope.as_deref()).await?,
        };
        client["issuer"] = issuer.clone().into();
        let (url, verifier) = http::authorization_url(
            &server,
            &metadata,
            client["client_id"].as_str().unwrap_or_default(),
            redirect,
            state,
            scope.as_deref(),
            resource.as_deref(),
        )?;
        let expires = store::now_ms() + TRANSACTION_TTL.as_millis() as u64;
        store::publish_pending(&self.store, &self.prefix, attempt, &url, baseline, expires, state).await?;
        on_url(url);
        // 只有「等人点授权」受 TTL 约束；code exchange 必须等到结束，不与超时竞速。
        let callback_url = tokio::select! {
            _ = cancel.cancelled() => bail!("MCP server {} OAuth authorization was cancelled", self.name),
            result = tokio::time::timeout(TRANSACTION_TTL, callback) => match result {
                Ok(Ok(result)) => result?,
                Ok(Err(_)) => bail!("MCP server {} OAuth callback listener stopped", self.name),
                Err(_) => bail!("MCP server {} OAuth authorization timed out", self.name),
            },
        };
        let callback_url = url::Url::parse(&callback_url)?;
        let param = |key: &str| callback_url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
        let code = param("code").or_else(|| param("authCode")).unwrap_or_default();
        // SDK validateAuthorizationResponseIssuer（RFC 9207）。
        if let Some(expected) = metadata["issuer"].as_str() {
            match param("iss") {
                None if metadata["authorization_response_iss_parameter_supported"] == true => {
                    bail!("Authorization response is missing the iss parameter")
                }
                Some(iss) if iss != expected => bail!("Authorization response issuer mismatch: {iss}"),
                _ => {}
            }
        }
        let params = vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code),
            ("code_verifier".into(), verifier),
            ("redirect_uri".into(), redirect.to_owned()),
        ];
        let mut tokens = http::token_request(&server, &metadata, params, &client, resource.as_deref()).await?;
        tokens["issuer"] = issuer.clone().into();
        let transaction = hex(&<sha2::Sha256 as sha2::Digest>::digest(state.as_bytes()));
        store::publish(&self.store, &self.prefix, &client, &tokens, Some(&issuer), &transaction).await?;
        Ok(Outcome::Authorized)
    }

    /// SDK `authInternal` 的 discovery 部分：共享缓存命中时补全缺失字段并在变化时回写，否则全量发现。
    async fn discover(&self, challenge_metadata_url: Option<&str>) -> Result<Value> {
        let cached = store::load_discovery(&self.store, &self.prefix, None).await?;
        let metadata_url = challenge_metadata_url
            .map(str::to_owned)
            .or_else(|| cached.as_ref().and_then(|c| c["resourceMetadataUrl"].as_str()).map(str::to_owned));
        let Some(cached) = cached else {
            let state = http::server_info(&self.url, metadata_url.as_deref()).await?;
            store::save_discovery(&self.store, &self.prefix, &state).await?;
            return Ok(state);
        };
        let mut state = json!({"authorizationServerUrl": cached["authorizationServerUrl"]});
        if let Some(url) = &metadata_url {
            state["resourceMetadataUrl"] = url.clone().into();
        }
        let server = cached["authorizationServerUrl"].as_str().unwrap_or_default();
        let metadata = match cached.get("authorizationServerMetadata") {
            Some(metadata) if metadata.is_object() => Some(metadata.clone()),
            _ => http::authorization_server_metadata(server).await?,
        };
        let resource = match cached.get("resourceMetadata") {
            Some(resource) if resource.is_object() => Some(resource.clone()),
            _ => match http::protected_resource(&self.url, metadata_url.as_deref()).await {
                Ok(resource) => Some(resource),
                Err(error) if error.is::<reqwest_mcp::Error>() => return Err(error),
                Err(_) => None,
            },
        };
        let changed = metadata.as_ref() != cached.get("authorizationServerMetadata")
            || resource.as_ref() != cached.get("resourceMetadata");
        if let Some(resource) = resource {
            state["resourceMetadata"] = resource;
        }
        if let Some(metadata) = metadata {
            state["authorizationServerMetadata"] = metadata;
        }
        if changed {
            store::save_discovery(&self.store, &self.prefix, &state).await?;
        }
        Ok(state)
    }
}

/// TS `createLocalhostOAuthCallbackServer`：state 不匹配回 400 并继续等待；`error` 参数立即失败。
async fn serve_callback(
    listener: tokio::net::TcpListener,
    path: String,
    state: String,
    sender: tokio::sync::oneshot::Sender<Result<String>>,
    stop: CancellationToken,
) {
    let mut sender = Some(sender);
    loop {
        let (mut socket, _) = tokio::select! {
            _ = stop.cancelled() => return,
            accepted = listener.accept() => match accepted { Ok(v) => v, Err(_) => continue },
        };
        let mut buffer = vec![0u8; 16 * 1024];
        let read = tokio::time::timeout(std::time::Duration::from_secs(10), socket.read(&mut buffer)).await;
        let Ok(Ok(count)) = read else { continue };
        let request = String::from_utf8_lossy(&buffer[..count]);
        let target = request.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/");
        let url = url::Url::parse(&format!("http://127.0.0.1{target}"));
        let (status, settle) = match &url {
            Err(error) => (500, Some(Err(anyhow!("{error}")))),
            Ok(url) if url.path() != path => (404, None),
            Ok(url) => {
                let param = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
                if param("state").unwrap_or_default() != state {
                    (400, None)
                } else if let Some(error) = param("error") {
                    (400, Some(Err(anyhow!("OAuth authorization was rejected by the authorization server: {error}"))))
                } else if param("authCode").or_else(|| param("code")).unwrap_or_default().is_empty() {
                    (400, Some(Err(anyhow!("OAuth callback is missing an authorization code."))))
                } else {
                    (200, Some(Ok(url.to_string())))
                }
            }
        };
        let text = if status == 200 { SUCCESS_TEXT } else { FAILURE_TEXT };
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            _ => "Internal Server Error",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
        if let Some(result) = settle
            && let Some(sender) = sender.take()
        {
            let _ = sender.send(result);
        }
    }
}
