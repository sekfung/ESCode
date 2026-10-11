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
    /// `timeZone` 在 `at_ms` 时刻相对 UTC 的偏移（ms，TS resolveTzOffsetMs）；测试时钟按 UTC。
    fn tz_offset_ms(&self, _time_zone: &str, _at_ms: i64) -> i64 {
        0
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
    /// 会话的 Gen UI 输出目录（TS resolveGenUiOutputDirectory + ensureGenUiOutputDirectory）：
    /// 仅桌面且 Host 提供输出根时存在，返回前已创建；创建失败即不声明。
    async fn gen_ui_directory(&self, _session_id: &str) -> Option<String> {
        None
    }
    async fn snapshot(
        &self,
        cancel: &CancellationToken,
    ) -> Result<zcode_cli_domain::prompt::PromptSnapshot>;
    async fn instructions(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Vec<zcode_cli_domain::prompt::InstructionSource>>;
}
