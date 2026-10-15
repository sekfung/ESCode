#![allow(clippy::disallowed_methods)] // 本 crate 是网络出口单入口：reqwest builder 只允许在这里构造。

//! 网络出口单入口（rust-unified-host.md P1 #6；质量报告 §6「Z 的 net crate 单入口」）。
//!
//! 三类出口共用同一条策略：**代理解析**（显式配置、`ESCODE_HTTP_PROXY`/`ESCODE_NO_PROXY`
//! 与捕获的宿主代理，见 `docs/specs/rust-net-proxy.md`）与**自定义 CA**（`ESCODE_AGENT_CA_CERT`，
//! 其次 `NODE_EXTRA_CA_CERTS`）。本 crate 是 `reqwest::Client::builder` / `reqwest_mcp::Client::builder`
//! 的唯一允许调用点（clippy `disallowed_methods` 强制），三处调用方各自保留自己的超时/重定向参数，
//! 出口策略只在这里改一次。
//!
//! 系统证书读取含阻塞 IO：客户端构建一律经受管阻塞线程（`Spawner::current().spawn_blocking`），
//! 不阻塞 stdio actor 的启动与取消。

use std::time::Duration;

use anyhow::Result;

pub use escode_cli_host::net_config::NetworkScope;

/// 出口用途（调用方各自的超时/重定向形状；代理策略三者相同）。
///
/// 修复：此前 `Model` 在未解析出代理时不调 `no_proxy()`，reqwest 便按**进程** env 的
/// `HTTP_PROXY`/`HTTPS_PROXY` 自行代理。TS 模型请求（`createNetworkProxyFetch`）未解析出代理即直连
/// 全局 `fetch`（Node `fetch` 不读这些变量）；且嵌入 Host 时进程 env 属于 Host、不是 Main 交给实例的
/// env 快照——本机设了 `HTTP_PROXY` 时连回环地址的模型服务也被送去代理。应走代理的来源
/// （设置页、`ESCODE_HTTP_PROXY`、捕获环境）已由 [`resolve`] 显式解析。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// 模型请求。
    Model,
    /// MCP HTTP/SSE：未命中时显式禁用系统代理（TS `createNetworkProxyFetch` 同）。
    Mcp,
    /// WebFetch / 插件下载：无代理即显式禁用（TS `resolveWebFetchProxyForRequest` 的 `direct` 同）。
    WebFetch,
}

/// 已解析的出口事实：代理 URL、是否命中 no_proxy、附加根证书。
#[derive(Clone, Debug, Default)]
pub struct Egress {
    pub proxy: Option<String>,
    pub no_proxy_matched: bool,
    pub extra_ca: Vec<Vec<u8>>,
}

/// 一次客户端构建的参数。
#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub purpose: Purpose,
    /// 目标 URL（决定 `no_proxy` 是否命中）。
    pub target: String,
    pub scope: NetworkScope,
    /// 整请求超时（None = 不设，交给上游的流式空闲超时）。
    pub timeout: Option<Duration>,
    /// 连接超时（MCP 用 15s；其余不设）。
    pub connect_timeout: Option<Duration>,
    /// 连接池空闲上限（模型 90s；其余默认）。
    pub pool_idle: Option<Duration>,
    /// TCP_NODELAY（模型开启）。
    pub tcp_nodelay: bool,
}

impl ClientOptions {
    pub fn new(purpose: Purpose, target: impl Into<String>) -> Self {
        Self {
            purpose,
            target: target.into(),
            scope: NetworkScope::Process,
            timeout: None,
            connect_timeout: None,
            pool_idle: None,
            tcp_nodelay: false,
        }
    }

    pub fn scope(mut self, scope: NetworkScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// 可选超时（`None` 保持不设）。
    pub fn apply_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    pub fn pool_idle(mut self, idle: Duration) -> Self {
        self.pool_idle = Some(idle);
        self
    }

    pub fn tcp_nodelay(mut self) -> Self {
        self.tcp_nodelay = true;
        self
    }
}

/// 解析出口策略（代理解析读实例 env 快照 + 用户 `network` 段；CA 读设置页自定义证书）。
pub fn resolve(target: &str, scope: NetworkScope) -> Result<Egress> {
    let resolution = escode_cli_domain::net_proxy::resolve_webfetch_proxy_for_request(
        target,
        &escode_cli_host::net_config::proxy_options(scope),
    );
    Ok(Egress {
        proxy: resolution.proxy_url,
        no_proxy_matched: resolution.no_proxy_matched,
        extra_ca: escode_cli_host::tls_ca::extra_ca_certificates(scope)?,
    })
}

/// 把出口策略应用到 reqwest builder（代理 → CA，顺序与 TS 的 TLS 配置一致）。
pub fn apply(
    mut builder: reqwest::ClientBuilder,
    egress: &Egress,
    _purpose: Purpose,
) -> Result<reqwest::ClientBuilder> {
    match &egress.proxy {
        Some(proxy) => builder = builder.proxy(reqwest::Proxy::all(proxy)?),
        // 未解析出代理即直连（不让 reqwest 回落到进程 env 的代理变量）。
        None => builder = builder.no_proxy(),
    }
    for bytes in &egress.extra_ca {
        let certificates = match reqwest::Certificate::from_pem_bundle(bytes) {
            Ok(certificates) => certificates,
            Err(_) => vec![reqwest::Certificate::from_der(bytes)?],
        };
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

/// 同上，应用到 reqwest_mcp builder。
pub fn apply_mcp(
    mut builder: reqwest_mcp::ClientBuilder,
    egress: &Egress,
    _purpose: Purpose,
) -> Result<reqwest_mcp::ClientBuilder> {
    match &egress.proxy {
        Some(proxy) => builder = builder.proxy(reqwest_mcp::Proxy::all(proxy)?),
        // 未解析出代理即直连（不让 reqwest 回落到进程 env 的代理变量）。
        None => builder = builder.no_proxy(),
    }
    for bytes in &egress.extra_ca {
        let certificates = match reqwest_mcp::Certificate::from_pem_bundle(bytes) {
            Ok(certificates) => certificates,
            Err(_) => vec![reqwest_mcp::Certificate::from_der(bytes)?],
        };
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

/// 构建 reqwest 客户端（受管阻塞线程内完成证书读取与 builder 构建）。
pub async fn client(options: ClientOptions) -> Result<reqwest::Client> {
    let ClientOptions {
        purpose,
        target,
        scope,
        timeout,
        connect_timeout,
        pool_idle,
        tcp_nodelay,
    } = options;
    escode_cli_core_api::scope::Spawner::current()
        .spawn_blocking(
            "net-http-client-build",
            move || -> Result<reqwest::Client> {
                let egress = resolve(&target, scope)?;
                let mut builder =
                    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
                if let Some(idle) = pool_idle {
                    builder = builder.pool_idle_timeout(idle);
                }
                if let Some(timeout) = connect_timeout {
                    builder = builder.connect_timeout(timeout);
                }
                if let Some(timeout) = timeout {
                    builder = builder.timeout(timeout);
                }
                builder = if tcp_nodelay {
                    builder.tcp_nodelay(true)
                } else {
                    builder
                };
                let builder = apply(builder, &egress, purpose)?;
                Ok(builder.build()?)
            },
        )
        .await?
}

/// 构建 reqwest_mcp 客户端（MCP 用 reqwest_mcp 的 Proxy/Certificate 类型）。
pub async fn mcp_client(options: ClientOptions) -> Result<reqwest_mcp::Client> {
    let ClientOptions {
        purpose,
        target,
        scope,
        timeout,
        connect_timeout,
        ..
    } = options;
    escode_cli_core_api::scope::Spawner::current()
        .spawn_blocking(
            "net-mcp-client-build",
            move || -> Result<reqwest_mcp::Client> {
                // reqwest_mcp 用 rustls-no-provider：构建前幂等安装 ring provider
                // （原先散在 mcp_hub；出口单入口后由本 crate 保证，调用方不必先记得装）。
                let _ = rustls::crypto::ring::default_provider().install_default();
                let egress = resolve(&target, scope)?;
                let mut builder =
                    reqwest_mcp::Client::builder().redirect(reqwest_mcp::redirect::Policy::none());
                if let Some(timeout) = connect_timeout {
                    builder = builder.connect_timeout(timeout);
                }
                if let Some(timeout) = timeout {
                    builder = builder.timeout(timeout);
                }
                let builder = apply_mcp(builder, &egress, purpose)?;
                Ok(builder.build()?)
            },
        )
        .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purpose_controls_the_absent_proxy_default() {
        // 三类用途无代理时都显式直连（行为由 tests/model_direct.rs 在设置了进程代理变量的环境里验证）。
        let direct = Egress {
            proxy: None,
            no_proxy_matched: false,
            extra_ca: Vec::new(),
        };
        let webfetch = apply(reqwest::Client::builder(), &direct, Purpose::WebFetch)
            .expect("webfetch applies")
            .build()
            .expect("builds");
        let model = apply(reqwest::Client::builder(), &direct, Purpose::Model)
            .expect("model applies")
            .build()
            .expect("builds");
        // 这里只保证入口可用且不 panic；直连行为见 tests/model_direct.rs。
        let _ = (webfetch, model);
        let matched = Egress {
            proxy: None,
            no_proxy_matched: true,
            extra_ca: Vec::new(),
        };
        apply(reqwest::Client::builder(), &matched, Purpose::Model)
            .expect("no_proxy 命中时禁用系统代理")
            .build()
            .expect("builds");
    }

    #[test]
    fn invalid_proxy_url_is_reported() {
        let broken = Egress {
            proxy: Some("not a url".to_owned()),
            no_proxy_matched: false,
            extra_ca: Vec::new(),
        };
        let error =
            apply(reqwest::Client::builder(), &broken, Purpose::Mcp).expect_err("非法代理必须失败");
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn client_builds_with_the_requested_timeouts() {
        let client = client(
            ClientOptions::new(Purpose::WebFetch, "https://example.com/")
                .timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(3)),
        )
        .await
        .expect("client builds");
        let mcp = mcp_client(
            ClientOptions::new(Purpose::Mcp, "https://example.com/mcp")
                .connect_timeout(Duration::from_secs(15)),
        )
        .await
        .expect("mcp client builds");
        let _ = (client, mcp);
    }
}
