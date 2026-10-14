//! 会话 shell 偏好的查询与提醒（从 tool_shell.rs 拆出）。

use crate::contract::{Event, EventSink};
use std::time::Duration;
use tokio::sync::oneshot;

/// 首个 Bash 前向会话 owner 请求用户终端偏好（TS `resolveInitialBashShellSelection`）；
/// 无 owner（fixture/测试）、Host 不支持或超时时按自动探测处理，见 docs/specs/rust-shell-selection.md。
pub(super) async fn shell_override(
    sink: Option<&EventSink>,
) -> Option<crate::shell_select::Override> {
    let sink = sink?;
    let (reply, rx) = oneshot::channel();
    sink.send(Event::ShellPreference { reply }).await.ok()?;
    let value = tokio::time::timeout(Duration::from_millis(15_000), rx)
        .await
        .ok()?
        .ok()??;
    crate::shell_select::parse_override(&value)
}

/// 会话当前的 shell 选择（与 Bash 执行同一 Host 偏好与解析）。
async fn session_shell_selection(
    sink: &crate::contract::EventSink,
) -> crate::shell_select::Selection {
    let over = shell_override(Some(sink)).await;
    let env: Vec<(String, String)> = std::env::vars().collect();
    crate::shell_select::resolve(
        crate::shell_select::Platform::current(),
        &env,
        over.as_ref(),
        &|p| std::path::Path::new(p).is_file(),
    )
}

/// 恢复会话首轮的 shell 提醒（docs/specs/rust-shell-resume-notice.md）。
pub(super) async fn resume_notice(
    sink: &crate::contract::EventSink,
    persisted: Option<&str>,
) -> Option<String> {
    crate::shell_select::resume_notice(&session_shell_selection(sink).await, persisted)
}

/// 系统提示词里的 Shell 名（与 Bash 执行同一选择）。
pub(super) async fn display_name(sink: &crate::contract::EventSink) -> String {
    crate::shell_select::display_name(&session_shell_selection(sink).await)
}
