//! 运行环境端口：时钟、请求期鉴权与工作区上下文（由 Host adapter 实现）。
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub trait RuntimeClock: Send + Sync {
    fn now(&self) -> u64;
    fn id(&self) -> String;
    /// 本地日期 `YYYY-MM-DD`（TS `formatLocalIsoDate`），供跨日 reminder；测试时钟可不提供。
    fn local_date(&self) -> Option<String> {
        None
    }
}
pub use RuntimeClock as Clock;

/// Request-scoped authentication. Implementations must never persist the
/// returned value in a session, queue, ACK or diagnostic record.
#[async_trait]
pub trait AuthPort: Send + Sync {
    async fn credentials(
        &self,
        session_id: &str,
        request_id: &str,
        workspace: &str,
    ) -> Result<Value>;
}

#[async_trait]
pub trait ContextPort: Send + Sync {
    fn desktop(&self) -> bool;
    async fn snapshot(
        &self,
        cancel: &CancellationToken,
    ) -> Result<zcode_cli_domain::prompt::PromptSnapshot>;
    async fn instructions(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Vec<zcode_cli_domain::prompt::InstructionSource>>;
}
