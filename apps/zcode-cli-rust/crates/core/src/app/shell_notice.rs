//! 恢复会话后的 shell 提醒（docs/specs/rust-shell-resume-notice.md）：冷恢复 / fork / 副屏 child 在本进程的首轮，
//! 按 TS announceSessionShellEnvironmentNoticeAfterResume 判定，插在本轮输入（及其前置提醒、child 分支提醒）之前。
use super::context::RunContext;
use crate::contract::{Event, EventSink, ToolPort};
use anyhow::Result;
use serde_json::{Value, json};

/// fork / 副屏 child 的分支提醒属于 child 的首条输入（Node 恢复 child 时把 shell 提醒放在它们之前）。
const CHILD_BOUNDARY_SOURCES: [&str; 2] = ["fork_notice", "selection_side_chat"];

/// 判定并记录提醒：owner 只记内存（`Event::ShellNotice`），运行副本在 projection 里拼进请求。
pub(super) async fn resolve(
    tools: &dyn ToolPort,
    history: &mut RunContext,
    sink: &EventSink,
) -> Result<()> {
    if history.turn.session_start != Some("resume") || history.shell_notice.is_some() {
        return Ok(());
    }
    let persisted = history.prompt_snapshot.as_ref().map(|s| s.shell.clone());
    let Some(text) = tools.shell_resume_notice(sink, persisted.as_deref()).await else {
        return Ok(());
    };
    let at = anchor(&history.messages);
    let message = json!({
        "role": "user",
        "content": crate::domain::plan_mode::wrap(&text),
        "_zcode_source": "shell_environment_change",
    });
    sink.send(Event::ShellNotice {
        at,
        message: message.clone(),
    })
    .await?;
    history.shell_notice = Some((at, message));
    Ok(())
}

/// 锚点：本轮输入起点，再越过紧邻其前的 child 分支提醒（App 差分实测的 Node 顺序）。
fn anchor(messages: &[Value]) -> usize {
    let mut at = super::turn_hooks::input_start(messages);
    while at > 0
        && CHILD_BOUNDARY_SOURCES
            .iter()
            .any(|source| messages[at - 1]["_zcode_source"] == *source)
    {
        at -= 1;
    }
    at
}
