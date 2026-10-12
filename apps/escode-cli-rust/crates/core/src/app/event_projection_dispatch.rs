//! `apply_event` 的前置分流：辅助会话、后台任务、run/turn 归属校验与旁路事件。
//! `None` 表示事件已在这里处理完，`Some` 携带继续投影所需的会话上下文。
use super::{Engine, Event, RunEvent};
use anyhow::{Result, bail};

pub(super) struct DispatchedRunEvent {
    pub(super) event: RunEvent,
    pub(super) id: String,
    pub(super) turn: String,
}

impl Engine {
    pub(super) async fn dispatch_run_event(
        &mut self,
        event: RunEvent,
    ) -> Result<Option<DispatchedRunEvent>> {
        if self.auxiliary_owned(&event.session_id) {
            self.auxiliary_event(event).await?;
            return Ok(None);
        }
        if let Event::ToolCleanupFailed(message) = event.event {
            let owned = self
                .active
                .get(&event.session_id)
                .is_some_and(|a| a.run_id == event.run_id)
                || self.sessions.get(&event.session_id).is_some_and(|s| {
                    s.background
                        .values()
                        .any(|task| task.run_id == event.run_id && task.status == "running")
                });
            if owned {
                bail!("{message}");
            }
            return Ok(None);
        }
        if let Event::Background { task, committed } = event.event {
            self.background_event(&event.session_id, &event.run_id, task, committed)
                .await?;
            return Ok(None);
        }
        let id = event.session_id.clone();
        let Some(active) = self.active.get(&id) else {
            return Ok(None);
        };
        if active.run_id != event.run_id {
            return Ok(None);
        }
        let cancelled = active.cancel.is_cancelled();
        if cancelled && !matches!(event.event, Event::Finished { .. }) {
            return Ok(None);
        }
        if matches!(
            event.event,
            Event::SkillsInitialized { .. }
                | Event::PromptInitialized { .. }
                | Event::ContextUsage(_) | Event::ShellNotice { .. } | Event::StreamRecovery { .. }
                | Event::CompactStarted { .. }
                | Event::CompactDone { .. }
        ) {
            self.context_event(&id, event.event).await?;
            return Ok(None);
        }
        let turn = active.turn_id.clone();
        if matches!(event.event, Event::FilePrepared { .. }) {
            self.file_checkpoint_event(&id, event.event).await?;
            return Ok(None);
        }
        if matches!(event.event, Event::Subagent { .. }) {
            self.subagent_event(&id, event.event).await?;
            return Ok(None);
        }
        if matches!(event.event, Event::GoalStep { .. } | Event::GoalVerdict { .. }) {
            self.goal_event(&id, event.event).await?;
            return Ok(None);
        }
        if let Event::HookContext { message, before_input, committed } = event.event {
            self.hook_context(&id, message, before_input, committed).await?;
            return Ok(None);
        }
        if let Event::Todos {
            call_id,
            write,
            reply,
        } = event.event
        {
            self.todo_tool(&id, &call_id, write, reply).await?;
            return Ok(None);
        }
        if let Event::TodoReminder { reply } = event.event {
            self.todo_reminder(&id, reply).await?;
            return Ok(None);
        }
        if let Event::PersistedNotice { message, reply } = event.event {
            self.persisted_notice(&id, message, reply).await?;
            return Ok(None);
        }
        if let Event::Question {
            call_id,
            input,
            reply,
        } = event.event
        {
            self.register_question(&id, &event.run_id, &call_id, *input, reply)
                .await?;
            return Ok(None);
        }
        if matches!(
            event.event,
            Event::PlanEnter { .. } | Event::PlanExit { .. }
        ) {
            self.plan_event(&id, &event.run_id, &turn, event.event)
                .await?;
            return Ok(None);
        }
        if let Event::StepBoundary { committed } = event.event {
            if let Some(messages) = self.drain_step_messages(&id, &turn).await? {
                let _ = committed.send(Some(messages));
                return Ok(None);
            }
            self.drain_guide(&id, &turn, committed).await?;
            return Ok(None);
        }
        if matches!(
            event.event,
            Event::HostRequest { .. }
                | Event::WorkflowSettled { .. } | Event::ActorRequest { .. }
                | Event::SessionContext { .. }
                | Event::WorkflowRunList { .. }
                | Event::ShellPreference { .. }
                | Event::MemoryPreference { .. }
                | Event::MemoryResolved(_)
                | Event::MemoryExtract(_)
                | Event::RequestAuth { .. }
        ) {
            self.owner_request(&id, &event.run_id, &turn, event.event);
            return Ok(None);
        }
        if matches!(event.event, Event::Finished { .. }) {
            self.cancel_auth(&id);
            self.notify_turn_ended(&id, &turn);
        }
        Ok(Some(DispatchedRunEvent { event, id, turn }))
    }
}
