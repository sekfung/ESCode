//! 插件域的副作用端口（P1 #9）。
//!
//! 抽 crate 后插件域不能再直接调用 tools 的实现：取消判定、HTTP 下载与受管阻塞线程
//! 统一从这里的 [`Ports`] 注入。默认实现只依赖 `core-api` / `net`，测试可替换为记录型
//! 端口而不启动真实网络。

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// 插件下载使用的网络出口视图。
pub use escode_cli_host::net_config::NetworkScope;

/// 插件域的外部副作用端口。
#[async_trait]
pub trait Ports: Send + Sync {
    /// 统一的取消语义（与 tools 的 `check_cancel` 同文案）。
    fn check_cancel(&self, cancel: &CancellationToken) -> Result<()>;

    /// 应用层下载客户端：代理/CA 策略统一在 `escode-cli-net`，调用方只给目标、超时与 scope。
    async fn http_client(
        &self,
        target: String,
        timeout: Duration,
        scope: NetworkScope,
    ) -> Result<reqwest::Client>;
}

/// 生产端口：保留既有行为，不读取插件域之外的业务事实。
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultPorts;

#[async_trait]
impl Ports for DefaultPorts {
    fn check_cancel(&self, cancel: &CancellationToken) -> Result<()> {
        if cancel.is_cancelled() {
            anyhow::bail!("Cancelled")
        }
        Ok(())
    }

    async fn http_client(
        &self,
        target: String,
        timeout: Duration,
        scope: NetworkScope,
    ) -> Result<reqwest::Client> {
        escode_cli_net::client(
            escode_cli_net::ClientOptions::new(escode_cli_net::Purpose::WebFetch, target)
                .scope(scope)
                .timeout(timeout),
        )
        .await
    }
}

/// 默认端口的快捷函数，供不想显式构造端口的内部调用点使用。
pub async fn proxied_client(
    target: String,
    timeout: Duration,
    scope: NetworkScope,
) -> Result<reqwest::Client> {
    DefaultPorts.http_client(target, timeout, scope).await
}
