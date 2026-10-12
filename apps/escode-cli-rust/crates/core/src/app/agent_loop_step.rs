//! 每步模型请求前的提醒装配：agent 目录增量与 Todo 提醒（TS appendRuntimeAgentListing / shouldRemind）。
use super::context::RunContext;
use crate::contract::{Event, EventSink};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) async fn append_step_reminders(
    sink: &EventSink,
    cancel: &CancellationToken,
    history: &mut RunContext,
    definitions: &[Value],
    continuations: usize,
    agent_listed: bool,
    profiles: &[crate::domain::subagent::Profile],
) -> Result<()> {
    // TS appendRuntimeAgentListing：输出续写之外的每步按历史已通告集合补发增量，先持久化再进请求。
    if continuations == 0
        && agent_listed
        && let Some((content, delta)) =
            crate::domain::agent_description::listing_delta(profiles, &history.messages, true)
    {
        use crate::domain::agent_description::{LISTING_DELTA_KEY, LISTING_SOURCE};
        let message = json!({
            "role": "user",
            "content": format!("<system-reminder>
{content}
</system-reminder>"),
            "_escode_source": LISTING_SOURCE,
            LISTING_DELTA_KEY: delta,
        });
        let (reply, receipt) = oneshot::channel();
        sink.send(Event::PersistedNotice {
            message: message.clone(),
            reply,
        })
        .await?;
        tokio::select! {biased;
            _=cancel.cancelled()=>bail!("Cancelled"),
            done=receipt=>done.context("Agent listing commit failed")?,
        }
        // 规范历史只追加（落库按长度增量）；Node 请求里本轮增量位于输入正文之前，由请求投影
        // `agent_description::order_listing_before_input` 还原该顺序。
        history.push(message);
    }
    if continuations == 0
        && definitions
            .iter()
            .any(|d| d["function"]["name"] == "TodoWrite")
        && crate::domain::todo::should_remind(&history.messages)
    {
        let (reply, receipt) = oneshot::channel();
        sink.send(Event::TodoReminder { reply }).await?;
        let message = tokio::select! {biased;
            _=cancel.cancelled()=>bail!("Cancelled"),
            message=receipt=>message.context("Todo reminder commit failed")?,
        };
        history.push(message);
    }
    Ok(())
}
