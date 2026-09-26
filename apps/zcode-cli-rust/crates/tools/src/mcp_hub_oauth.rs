//! MCP OAuth 交互授权编排（docs/specs/rust-mcp-oauth.md「状态与编排」，对齐 TS `openServerConnection` catch 分支与
//! `waitForSharedConnection`）：授权事务在后台运行（寿命 300s），调用方只按自身预算等待；
//! 完成后重连一次，并在 hub 状态锁内同时写入最终状态、连接并移除任务，避免并发 `prepare` 用旧快照覆盖。
use super::super::{
    mcp_config::{self, Server},
    mcp_connection::Connection,
    mcp_oauth_credentials::{Auth, ClientCredentials},
    mcp_oauth_flow::{AuthorizationRequired, OAuth, Outcome},
};
use super::{Hub, State};
use crate::domain::mcp_oauth as rules;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use zcode_cli_host::credential_store::CredentialStore;

/// Session 等待授权的预算（TS `MCP_SESSION_OAUTH_AUTHORIZATION_TIMEOUT_MS`）；超时后带着授权 URL 继续，事务不中断。
pub(super) const SESSION_BUDGET: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(super) enum Phase {
    Starting,
    Url(Value),
    /// Err 为 failureKind。
    Done(Result<Arc<Connection>, String>),
}
pub(super) struct Task {
    pub raw: Value,
    pub phase: watch::Receiver<Phase>,
}

/// TS `createOAuthClientProvider`：authorization_code（含未写 oauth 的 http/sse）走共享凭据，
/// client_credentials 走内存 token；官方鉴权仍为 not_authenticated。
pub(super) fn oauth_for(server: &Server, store: impl FnOnce() -> Arc<CredentialStore>) -> Result<Option<Auth>> {
    if server.transport == "stdio" {
        return Ok(None);
    }
    if let Some(config) = rules::code_config(&server.raw, &server.transport) {
        let url = server.raw["url"].as_str().unwrap_or_default();
        return Ok(Some(Auth::Code(Arc::new(OAuth::new(store(), &server.name, url, config)))));
    }
    let official = server.raw.get("auth").is_some();
    match ClientCredentials::from_config(&server.name, &server.raw) {
        Some(credentials) if !official => Ok(Some(Auth::Credentials(Arc::new(credentials)))),
        _ if official || server.raw.get("oauth").is_some() => bail!("not_authenticated"),
        _ => Ok(None),
    }
}

pub(super) fn connected(server: &Server, connection: &Connection, count: usize) -> Value {
    let mut status = mcp_config::status(server, "connected", count, None);
    status["protocolEra"] = if connection.modern { "modern" } else { "legacy" }.into();
    status
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start(
    state: Arc<RwLock<State>>,
    server: Server,
    key: String,
    http: Option<reqwest_mcp::Client>,
    oauth: Arc<OAuth>,
    trigger: AuthorizationRequired,
    stop: CancellationToken,
) -> Arc<Task> {
    let (tx, rx) = watch::channel(Phase::Starting);
    let task = Arc::new(Task {
        raw: server.raw.clone(),
        phase: rx,
    });
    state.write().unwrap().auth.insert(key.clone(), task.clone());
    tokio::spawn(async move {
        let on_url = {
            let (tx, state, server) = (tx.clone(), state.clone(), server.clone());
            Arc::new(move |url: String| {
                let mut status = mcp_config::status(&server, "connecting", 0, None);
                status["authorization"] = json!({
                    "type": "oauth_authorization_code",
                    "authorizationUrl": url,
                    "startedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                });
                state.write().unwrap().statuses.insert(server.name.clone(), status.clone());
                tx.send_replace(Phase::Url(status));
            })
        };
        let result = match oauth.authorize(&trigger, on_url, &stop).await {
            // 授权后只重连一次（TS oauthAuthorizationAttempted）；仍失败按普通连接失败分类。
            Outcome::Authorized | Outcome::AlreadyAuthorized => Connection::open(&server, http, Some(Auth::Code(oauth)), &stop)
                .await
                .map(Arc::new)
                .map_err(|error| failure_kind(&error).into()),
            Outcome::Pending => {
                eprintln!("zcode-cli-rust: MCP server {} OAuth authorization is still in progress; complete it in the browser and reconnect", server.name);
                Err("oauth_authorization_failed".to_owned())
            }
            Outcome::Failed(error) => {
                eprintln!("zcode-cli-rust: MCP server {} OAuth authorization failed: {error:#}", server.name);
                Err("oauth_authorization_failed".to_owned())
            }
        };
        let old = {
            let mut state = state.write().unwrap();
            let old = match &result {
                Ok(connection) => {
                    let status = connected(&server, connection, connection.tools.len());
                    state.statuses.insert(server.name.clone(), status);
                    state.connections.insert(key.clone(), connection.clone())
                }
                Err(kind) => {
                    let status = mcp_config::status(&server, "failed", 0, Some(kind));
                    state.statuses.insert(server.name.clone(), status);
                    None
                }
            };
            state.auth.remove(&key);
            old
        };
        if let Some(old) = old {
            let _ = old.close().await;
        }
        tx.send_replace(Phase::Done(result));
    });
    task
}

/// 调用方等待：设置页（`until_url`）拿到授权 URL 即返回；session 最多等 `SESSION_BUDGET`。
pub(super) async fn wait(task: &Task, until_url: bool, cancel: &CancellationToken) -> Phase {
    let mut phase = task.phase.clone();
    let deadline = tokio::time::Instant::now() + if until_url { super::super::mcp_oauth_flow::TRANSACTION_TTL } else { SESSION_BUDGET };
    loop {
        let current = phase.borrow_and_update().clone();
        match &current {
            Phase::Done(_) => return current,
            Phase::Url(_) if until_url => return current,
            _ => {}
        }
        tokio::select! {
            _ = cancel.cancelled() => return current,
            _ = tokio::time::sleep_until(deadline) => return current,
            changed = phase.changed() => if changed.is_err() { return phase.borrow().clone() },
        }
    }
}

impl Hub {
    /// 打开连接；需要交互授权时启动（或复用进行中的）后台授权事务，并按调用方预算等待。
    pub(super) async fn open(
        &self,
        server: &Server,
        key: &str,
        until_url: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<Arc<Connection>>> {
        let http = (server.transport != "stdio").then(|| {
            self.http
                .get_or_init(|| {
                    reqwest_mcp::Client::builder()
                        .redirect(reqwest_mcp::redirect::Policy::none())
                        .connect_timeout(std::time::Duration::from_secs(15))
                        .build()
                        .expect("MCP HTTP client")
                })
                .clone()
        });
        let credentials = || {
            self.credentials
                .get_or_init(|| Arc::new(zcode_cli_host::credential_store::CredentialStore::from_environment()))
                .clone()
        };
        // 同配置的授权仍在进行时共享该事务：重连会作废浏览器里已打开的授权 URL 与 PKCE/state（TS 同）。
        let running = self.state.read().unwrap().auth.get(key).cloned();
        let task = match running.filter(|t| t.raw == server.raw) {
            Some(task) => task,
            None => {
                let oauth = oauth_for(server, credentials)?;
                match Connection::open(server, http.clone(), oauth.clone(), cancel).await {
                    Ok(connection) => return Ok(Some(Arc::new(connection))),
                    Err(error) => match (error.downcast::<AuthorizationRequired>(), oauth) {
                        (Ok(trigger), Some(Auth::Code(oauth))) => start(
                            self.state.clone(),
                            server.clone(),
                            key.into(),
                            http,
                            oauth,
                            trigger,
                            self.stop.child_token(),
                        ),
                        (Ok(_), _) => anyhow::bail!("not_authenticated"),
                        (Err(error), _) => return Err(error),
                    },
                }
            }
        };
        match wait(&task, until_url, cancel).await {
            Phase::Done(Ok(connection)) => Ok(Some(connection)),
            Phase::Done(Err(kind)) => Err(anyhow::anyhow!(kind)),
            Phase::Url(status) => Err(PendingAuthorization(status).into()),
            Phase::Starting => Err(PendingAuthorization(mcp_config::status(server, "connecting", 0, None)).into()),
        }
    }
}

/// 授权进行中：以 connecting（含授权 URL）状态返回，事务继续在后台运行。
#[derive(Debug)]
pub(super) struct PendingAuthorization(pub Value);
impl std::fmt::Display for PendingAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP OAuth authorization pending")
    }
}
impl std::error::Error for PendingAuthorization {}
pub(super) fn failure_kind(error: &anyhow::Error) -> &'static str {
    match error.to_string().as_str() {
        "config_invalid" => "config_invalid",
        "not_authenticated" => "not_authenticated",
        "connection_timeout" => "connection_timeout",
        "process_start_failed" => "process_start_failed",
        "tool_list_failed" => "tool_list_failed",
        "oauth_authorization_failed" => "oauth_authorization_failed",
        _ => "protocol_negotiation_failed",
    }
}
