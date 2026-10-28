//! 「run 还欠着决定、主代理闲着」的提醒接线（docs/dynamic-workflow/transcript-and-notifications.md
//! 「Reminding an idle main agent」；纯时钟在 `domain::workflow_blocked_reminder`）。
//!
//! Engine 每 1s tick 一次：按会话算空闲、从 `workflow_runs` 投影读在飞 run 与开集，到点后走与
//! 其它 run 通知同一条 `workflow_settled` → `deliver_workflow_notices` 投递路径，不新增队列。

use super::Engine;
use crate::domain::workflow_blocked_reminder::{
    BlockedReminder, WorkflowOpenDecisions, blocked_meta, format_blocked_notification,
};
use anyhow::Result;
use serde_json::json;

impl Engine {
    pub(super) async fn workflow_blocked_tick(&mut self) -> Result<()> {
        let now = self.clock.now();
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        let mut due: Vec<(String, BlockedReminder)> = Vec::new();
        for id in ids {
            if self.closed.contains(&id) {
                self.workflow_reminders.remove(&id);
                continue;
            }
            let (idle, live) = {
                let Some(session) = self.sessions.get(&id) else {
                    continue;
                };
                let idle = !session.running()
                    && session.auto_drain
                    && session.queue.is_empty()
                    && session.settings_turns.is_empty();
                (idle, live_runs(session))
            };
            let reminders = {
                let clock = self.workflow_reminders.entry(id.clone()).or_default();
                if !idle {
                    clock.session_busy();
                    continue;
                }
                clock.session_idle(now);
                clock.due(now, &live)
            };
            for reminder in reminders {
                due.push((id.clone(), reminder));
            }
        }
        for (id, reminder) in due {
            self.enqueue_blocked_reminder(&id, reminder).await?;
        }
        Ok(())
    }

    async fn enqueue_blocked_reminder(
        &mut self,
        id: &str,
        reminder: BlockedReminder,
    ) -> Result<()> {
        let label = self
            .sessions
            .get(id)
            .and_then(|session| session.workflow_runs.as_ref())
            .and_then(|runs| runs["runs"].as_array())
            .and_then(|runs| {
                runs.iter()
                    .find(|run| run["runId"] == reminder.run_id.as_str())
            })
            .and_then(|run| run["label"].as_str())
            .unwrap_or(&reminder.run_id)
            .to_owned();
        // V4 workflowRuns 投影不带 scriptPath；有则附上，没有时文案退回 GetWorkflowRun。
        let draft = self
            .sessions
            .get(id)
            .and_then(|session| session.workflow_runs.as_ref())
            .and_then(|runs| runs["runs"].as_array())
            .and_then(|runs| {
                runs.iter()
                    .find(|run| run["runId"] == reminder.run_id.as_str())
            })
            .and_then(|run| run["scriptPath"].as_str())
            .map(str::to_owned);
        let text = format_blocked_notification(&label, &reminder, draft.as_deref());
        let notice = json!({
            "kind": "workflowBlockedReminder",
            "taskId": reminder.run_id,
            "noticeId": format!("workflow-blocked:{}:{}", reminder.run_id, reminder.ordinal),
            "text": text,
            "steer": false,
            "originMeta": {
                "backgroundSource": "workflow",
                "title": label,
                "workId": reminder.run_id,
                "workflowBlocked": blocked_meta(&reminder.open, reminder.now),
            },
        });
        self.workflow_settled(id, notice).await
    }
}

/// 本会话在飞的 workflow run 与当前开集；只读 `workflow_runs` 投影，不恢复 runtime。
fn live_runs(session: &crate::domain::session::Session) -> Vec<(String, WorkflowOpenDecisions)> {
    session
        .workflow_runs
        .as_ref()
        .and_then(|runs| runs["runs"].as_array())
        .map(|runs| {
            runs.iter()
                .filter(|run| run["status"] == "running")
                .filter_map(|run| {
                    let run_id = run["runId"].as_str()?.to_owned();
                    let questions = run["pendingQuestions"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let holes = run["holes"]
                        .as_array()
                        .map(|holes| {
                            holes
                                .iter()
                                .filter(|hole| hole["state"] == "waiting")
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    Some((run_id, WorkflowOpenDecisions { questions, holes }))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> crate::domain::session::Session {
        crate::domain::session::Session::new(
            "s".into(),
            "w".into(),
            "p".into(),
            "m".into(),
            String::new(),
            "e".into(),
            0,
        )
    }

    #[test]
    fn live_runs_only_running_and_waiting_holes() {
        let mut session = session();
        session.workflow_runs = Some(json!({"runs":[
            {"runId":"run-1","status":"running","pendingQuestions":[{"qid":"q1"}],"holes":[{"siteId":"h1","state":"waiting"},{"siteId":"h2","state":"filled"}]},
            {"runId":"run-2","status":"completed","pendingQuestions":[{"qid":"q2"}]}
        ]}));
        let live = live_runs(&session);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].0, "run-1");
        assert_eq!(live[0].1.questions.len(), 1);
        assert_eq!(live[0].1.holes.len(), 1);
        assert_eq!(live[0].1.holes[0]["siteId"], "h1");
    }
}
