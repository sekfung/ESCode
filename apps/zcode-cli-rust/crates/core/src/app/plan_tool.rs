//! EnterPlanMode / ExitPlanMode 的 loop 侧：只把调用交给会话 owner 并等待结果（owner 持有 plan 状态与审批）。
use crate::contract::{Event, EventSink, ToolOutput};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub(super) async fn execute(
    name: &str,
    call: &str,
    args: Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let (reply, receipt) = tokio::sync::oneshot::channel();
    let event = if name == crate::domain::plan_mode::ENTER_PLAN_MODE {
        Event::PlanEnter { reply }
    } else {
        Event::PlanExit {
            call_id: call.into(),
            input: args,
            reply,
        }
    };
    sink.send(event).await?;
    tokio::select! {biased;
        _=cancel.cancelled()=>bail!("{name} was cancelled before plan mode changed"),
        output=receipt=>output.context("Plan mode owner stopped before the result was committed"),
    }
}
