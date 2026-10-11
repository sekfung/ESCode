//! MCP HTTP 客户端：与 TS `adapters/src/mcp/network.ts` 的 `createMcpTransportFetch` 同一条网络策略
//! （`createNetworkProxyFetch`）——按目标 URL 解析代理（显式/`ESCODE_HTTP_PROXY`/`ESCODE_NO_PROXY`/
//! 捕获的宿主代理），并信任设置页的自定义 CA。见 docs/specs/rust-net-proxy.md。
use anyhow::Result;

/// 为一个 MCP HTTP/SSE 端点构造客户端；`target` 是该服务器的 URL（决定 `no_proxy` 是否命中）。
pub(super) fn client(target: &str) -> Result<reqwest_mcp::Client> {
    builder(target, None)
}

/// 同上，但带整请求超时（OAuth 令牌/发现端点用 60s）。
pub(super) fn client_with_timeout(
    target: &str,
    timeout: std::time::Duration,
) -> Result<reqwest_mcp::Client> {
    builder(target, Some(timeout))
}

fn builder(target: &str, timeout: Option<std::time::Duration>) -> Result<reqwest_mcp::Client> {
    let resolution = escode_cli_domain::net_proxy::resolve_webfetch_proxy_for_request(
        target,
        &escode_cli_host::net_config::proxy_options(escode_cli_host::net_config::NetworkScope::Process),
    );
    let mut builder = reqwest_mcp::Client::builder()
        .redirect(reqwest_mcp::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(15));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(proxy) = resolution.proxy_url {
        builder = builder.proxy(reqwest_mcp::Proxy::all(proxy)?);
    } else if resolution.no_proxy_matched {
        builder = builder.no_proxy();
    }
    for bytes in escode_cli_host::tls_ca::extra_ca_certificates(escode_cli_host::net_config::NetworkScope::Process)? {
        let certificates = match reqwest_mcp::Certificate::from_pem_bundle(&bytes) {
            Ok(certificates) => certificates,
            Err(_) => vec![reqwest_mcp::Certificate::from_der(&bytes)?],
        };
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder.build()?)
}
