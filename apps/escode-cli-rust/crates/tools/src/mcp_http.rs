//! MCP HTTP 客户端：与 TS `adapters/src/mcp/network.ts` 的 `createMcpTransportFetch` 同一条网络策略
//! （`createNetworkProxyFetch`）——按目标 URL 解析代理（显式/`ESCODE_HTTP_PROXY`/`ESCODE_NO_PROXY`/
//! 捕获的宿主代理），并信任设置页的自定义 CA。见 docs/specs/rust-net-proxy.md。
use anyhow::Result;

/// 为一个 MCP HTTP/SSE 端点构造客户端；`target` 是该服务器的 URL（决定 `no_proxy` 是否命中）。
pub(super) async fn client(target: &str) -> Result<reqwest_mcp::Client> {
    builder(target, None).await
}

/// 同上，但带整请求超时（OAuth 令牌/发现端点用 60s）。
pub(super) async fn client_with_timeout(
    target: &str,
    timeout: std::time::Duration,
) -> Result<reqwest_mcp::Client> {
    builder(target, Some(timeout)).await
}

async fn builder(
    target: &str,
    timeout: Option<std::time::Duration>,
) -> Result<reqwest_mcp::Client> {
    // 出口策略（代理 + 自定义 CA）统一在 crates/net：与 TS `createNetworkProxyFetch` 同一解析，
    // MCP 的 15s 连接超时与可选整请求超时由本处给出（rust-unified-host.md P1 #6）。
    escode_cli_net::mcp_client(
        escode_cli_net::ClientOptions::new(escode_cli_net::Purpose::Mcp, target)
            .connect_timeout(std::time::Duration::from_secs(15))
            .apply_timeout(timeout),
    )
    .await
}
