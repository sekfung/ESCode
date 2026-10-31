//! 每步模型请求前的提醒装配：agent 目录增量、Todo 提醒与 ToolSearch 目录公告
//! （TS appendRuntimeAgentListing / shouldRemind / buildToolSearchAnnouncement）。
use super::context::RunContext;
use crate::contract::{Event, EventSink};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// 本轮工具面事实：可见目录、注册目录、续写计数与 ToolSearch 决策。
pub(super) struct StepReminderFacts<'a> {
    pub(super) definitions: &'a [Value],
    pub(super) registered: &'a [Value],
    pub(super) continuations: usize,
    pub(super) agent_listed: bool,
    pub(super) profiles: &'a [crate::domain::subagent::Profile],
    pub(super) tool_search: Option<&'a super::agent_loop_surface::ToolSearchContext>,
}

pub(super) async fn append_step_reminders(
    sink: &EventSink,
    cancel: &CancellationToken,
    history: &mut RunContext,
    facts: StepReminderFacts<'_>,
) -> Result<()> {
    let StepReminderFacts {
        definitions,
        registered,
        continuations,
        agent_listed,
        profiles,
        tool_search,
    } = facts;
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
    // TS buildToolSearchAnnouncement：目录事实来自本轮 catalog，可用性对照完整注册目录；
    // 无增量时不写公告，因此重复轮天然幂等。纯 Continue 不生成公告（上游等下一次输入或
    // 工具批次），新增的延迟工具留到 continuations == 0 的步骤再通告。
    if continuations == 0
        && let Some(search) = tool_search
        && let Some(message) = crate::domain::tool_search_announcement::build(
            &history.messages,
            &search
                .catalog
                .iter()
                .filter(|candidate| candidate.deferred)
                .map(|candidate| candidate.name.clone())
                .collect::<Vec<_>>(),
            &registered
                .iter()
                .filter_map(|definition| definition["function"]["name"].as_str().map(str::to_owned))
                .collect::<HashSet<_>>(),
            search.mode,
        )
    {
        let (reply, receipt) = oneshot::channel();
        sink.send(Event::PersistedNotice {
            message: message.clone(),
            reply,
        })
        .await?;
        tokio::select! {biased;
            _=cancel.cancelled()=>bail!("Cancelled"),
            done=receipt=>done.context("Tool search announcement commit failed")?,
        }
        history.push(message);
    }
    Ok(())
}
