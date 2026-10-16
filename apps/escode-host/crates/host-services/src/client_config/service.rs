//! `client-config.getSnapshot` 的请求 + 缓存（TS `clientConfigService.ts: getSnapshot`，spec §2/§4）。
//!
//! 缓存的唯一所有者是 `ClientConfigService`：按 URL（含 endpoint/app_version/platform）分条目，1 h TTL；
//! 同条目的并发请求合并为一次（失败时所有等待者拿到同一个错误，且不写缓存）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use serde_json::Value;
use url::Url;

use super::snapshot::{ClientConfigSnapshot, parse_snapshot};
use crate::api_client::{ApiClient, ApiError, GetOptions};
use crate::provider::normalize_endpoint_origin;

pub(super) const CACHE_TTL_MS: u64 = 60 * 60 * 1000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 失败的类别决定跨 RPC 的错误名（`ApiError` / `Error` / `SyntaxError` / `ZodError`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Api(ApiError),
    Plain(String),
    /// `response.json()` 失败（TS 抛 `SyntaxError`）。
    Syntax(String),
    /// `clientConfigReadOptionsSchema.parse` 失败（TS 抛 `ZodError`）。
    Options(String),
}

impl ConfigError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Api(_) => "ApiError",
            Self::Plain(_) => "Error",
            Self::Syntax(_) => "SyntaxError",
            Self::Options(_) => "ZodError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Api(error) => &error.message,
            Self::Plain(message) | Self::Syntax(message) | Self::Options(message) => message,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api(error) => error.status,
            _ => None,
        }
    }
}

type Pending = Shared<BoxFuture<'static, Result<ClientConfigSnapshot, ConfigError>>>;
pub(super) type RawPending = Shared<BoxFuture<'static, Result<Value, ConfigError>>>;

#[derive(Default)]
struct Entry {
    snapshot: Option<ClientConfigSnapshot>,
    expires_at: u64,
    pending: Option<Pending>,
}

/// 动态工作流读取用的原始信封缓存（TS 另有一份，与快照缓存共存；它**不按 endpoint 分条目**，照 TS 原样）。
#[derive(Default)]
pub(super) struct RawEnvelope {
    pub(super) payload: Option<Value>,
    pub(super) expires_at: u64,
    pub(super) pending: Option<RawPending>,
}

/// 动态工作流的本地输入（env 覆盖在进程启动时取一次；用户选择每次从设置文件读）。
#[derive(Default, Clone)]
pub(super) struct DynamicInputs {
    pub(super) env_override: Option<Value>,
    pub(super) settings_home: Option<std::path::PathBuf>,
}

pub(super) struct Inner {
    pub(super) dynamic: DynamicInputs,
    pub(super) raw: Mutex<RawEnvelope>,
    pub(super) api: Arc<ApiClient>,
    app_version: String,
    platform: String,
    pub(super) clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    request_timeout: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

#[derive(Clone)]
pub struct ClientConfigService {
    pub(super) inner: Arc<Inner>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `clientConfigReadOptionsSchema`：`{forceRefresh?: boolean}`（未知键忽略；`null`/数组/非布尔都是 ZodError）。
pub fn parse_read_options(options: Option<&Value>) -> Result<bool, ConfigError> {
    let invalid = |reason: &str| ConfigError::Options(format!("Invalid options: {reason}"));
    match options {
        None => Ok(false),
        Some(Value::Object(map)) => match map.get("forceRefresh") {
            None => Ok(false),
            Some(Value::Bool(force)) => Ok(*force),
            Some(_) => Err(invalid("forceRefresh must be a boolean")),
        },
        Some(_) => Err(invalid("expected object")),
    }
}

impl ClientConfigService {
    pub fn new(api: Arc<ApiClient>, app_version: String, platform: String) -> Self {
        Self {
            inner: Arc::new(Inner {
                dynamic: DynamicInputs::default(),
                raw: Mutex::default(),
                api,
                app_version,
                platform,
                clock: Arc::new(now_ms),
                request_timeout: REQUEST_TIMEOUT,
                entries: Mutex::default(),
            }),
        }
    }

    #[cfg(test)]
    pub fn with_clock_and_timeout(
        self,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        request_timeout: Duration,
    ) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(Inner {
                dynamic: inner.dynamic.clone(),
                raw: Mutex::default(),
                api: Arc::clone(&inner.api),
                app_version: inner.app_version.clone(),
                platform: inner.platform.clone(),
                clock,
                request_timeout,
                entries: Mutex::default(),
            }),
        }
    }

    /// 配置动态工作流的本地输入（装配时调用一次）。
    #[must_use]
    pub fn with_dynamic_workflow(
        self,
        env_override: Option<String>,
        settings_home: Option<std::path::PathBuf>,
    ) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(Inner {
                dynamic: DynamicInputs {
                    env_override: env_override.map(Value::String),
                    settings_home,
                },
                raw: Mutex::default(),
                api: Arc::clone(&inner.api),
                app_version: inner.app_version.clone(),
                platform: inner.platform.clone(),
                clock: Arc::clone(&inner.clock),
                request_timeout: inner.request_timeout,
                entries: Mutex::default(),
            }),
        }
    }

    /// 当前 Endpoint 下的请求 URL（每次解析：设置里的覆盖值随时可变）。
    pub(super) async fn request_url(&self) -> Result<Url, ConfigError> {
        let endpoint = self
            .inner
            .api
            .endpoint_origin()
            .await
            .map_err(ConfigError::Api)?;
        let origin = normalize_endpoint_origin(&endpoint)
            .map_err(|error| ConfigError::Plain(format!("{error:#}")))?;
        let mut url = Url::parse(&origin).map_err(|error| ConfigError::Plain(error.to_string()))?;
        url.set_path("/api/v1/client/configs");
        url.query_pairs_mut()
            .append_pair("app_version", &self.inner.app_version)
            .append_pair("platform", &self.inner.platform);
        Ok(url)
    }

    pub async fn get_snapshot(
        &self,
        force_refresh: bool,
    ) -> Result<ClientConfigSnapshot, ConfigError> {
        let url = self.request_url().await?;
        let key = url.to_string();
        let pending = {
            let mut entries = lock(&self.inner.entries);
            let entry = entries.entry(key.clone()).or_default();
            if !force_refresh
                && let Some(snapshot) = &entry.snapshot
                && entry.expires_at > (self.inner.clock)()
            {
                return Ok(snapshot.clone());
            }
            // 已有在途请求就加入它（即使是 forceRefresh：TS 同）。
            entry
                .pending
                .get_or_insert_with(|| {
                    let inner = Arc::clone(&self.inner);
                    let key = key.clone();
                    async move {
                        let result = fetch_snapshot(&inner, &url).await;
                        let mut entries = lock(&inner.entries);
                        let entry = entries.entry(key).or_default();
                        if let Ok(snapshot) = &result {
                            entry.snapshot = Some(snapshot.clone());
                            entry.expires_at = (inner.clock)() + CACHE_TTL_MS;
                        }
                        entry.pending = None;
                        result
                    }
                    .boxed()
                    .shared()
                })
                .clone()
        };
        pending.await
    }
}

/// TS `fetchResponseJson`：15 s 总预算内取 `client/configs` 的 JSON。
pub(super) async fn fetch_payload(inner: &Inner, url: &Url) -> Result<Value, ConfigError> {
    let fetch = async {
        let response = inner
            .api
            .get(url.as_str(), &GetOptions::default())
            .await
            .map_err(ConfigError::Api)?;
        if !response.ok() {
            return Err(ConfigError::Plain(format!(
                "Public client config HTTP {}",
                response.status
            )));
        }
        // 设置里的 endpoint 可能在两次 await 之间变化；不能把新地址的响应缓存到旧地址。
        if !response.url.is_empty() && response.url != url.as_str() {
            return Err(ConfigError::Plain(
                "Public client config request context changed".to_owned(),
            ));
        }
        serde_json::from_slice::<Value>(&response.body)
            .map_err(|error| ConfigError::Syntax(error.to_string()))
    };
    tokio::time::timeout(inner.request_timeout, fetch)
        .await
        .map_err(|_| ConfigError::Plain("Public client config request timed out".to_owned()))?
}

/// `fetch_payload` + `parseClientConfigSnapshot`。
async fn fetch_snapshot(inner: &Inner, url: &Url) -> Result<ClientConfigSnapshot, ConfigError> {
    let payload = fetch_payload(inner, url).await?;
    parse_snapshot(&payload).map_err(ConfigError::Plain)
}
