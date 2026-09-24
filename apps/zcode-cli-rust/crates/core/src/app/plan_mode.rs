//! plan 模式的会话 owner 侧：planEnabled、ExitPlanMode 审批交互与审批后的追加消息。
//! 时序与规则见 docs/specs/rust-plan-mode.md。
use super::Engine;
use crate::contract::{Event, ToolControl, ToolOutput};
use crate::domain::{
    permission_options::{ask_reason, denied_content},
    plan_mode::{self, Decision},
    protocol::Command,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::oneshot;

pub(super) struct WaitingPlan {
    pub session: String,
    pub run: String,
    pub call_id: String,
    pub plan: String,
    pub reply: oneshot::Sender<ToolOutput>,
}

fn failed(content: String, control: ToolControl) -> ToolOutput {
    ToolOutput {
        failed: true,
        content,
        data: Value::Null,
        display: None,
        control,
    }
}

impl Engine {
    pub(super) async fn plan_event(
        &mut self,
        id: &str,
        run: &str,
        turn: &str,
        event: Event,
    ) -> Result<()> {
        match event {
            Event::PlanEnter { reply } => {
                let s = self.sessions.get_mut(id).context("Session unavailable")?;
                s.plan_enabled = true;
                s.revision += 1;
                self.publish(id, vec![])?;
                self.persist(id, None).await?;
                let _ = reply.send(ToolOutput::text(plan_mode::enter_result()));
                Ok(())
            }
            Event::PlanExit {
                call_id,
                input,
                reply,
            } => {
                self.register_plan_exit(id, run, turn, call_id, input, reply)
                    .await
            }
            _ => Ok(()),
        }
    }

    async fn register_plan_exit(
        &mut self,
        id: &str,
        run: &str,
        turn: &str,
        call_id: String,
        input: Value,
        reply: oneshot::Sender<ToolOutput>,
    ) -> Result<()> {
        let s = self.sessions.get_mut(id).context("Session unavailable")?;
        if !s.plan_enabled {
            let _ = reply.send(failed(
                plan_mode::NOT_IN_PLAN.into(),
                ToolControl::default(),
            ));
            return Ok(());
        }
        let interaction = self.clock.id();
        let now = self.clock.now();
        let prompt = ask_reason("tool.userInteraction", plan_mode::EXIT_PLAN_MODE);
        let mut deltas = vec![];
        let mut anchor = Value::Null;
        if let Some(row) = s
            .rows
            .iter_mut()
            .find(|r| r["turnId"] == turn && r["toolCallId"] == call_id.as_str())
        {
            row["status"] = "pendingApproval".into();
            row["approvalInteractionId"] = interaction.clone().into();
            anchor = row["rowId"].clone();
            deltas.push(json!({"op":"row.upserted","row":row}));
        }
        s.pending.push(json!({
            "interactionId": interaction,
            "kind": "userInput",
            "anchorRowId": anchor,
            "createdAt": now,
            "payload": plan_mode::approval_payload(&call_id, &input, &prompt),
        }));
        s.revision += 1;
        s.updated_at = now;
        self.plan_exits.insert(
            interaction,
            WaitingPlan {
                session: id.into(),
                run: run.into(),
                call_id,
                plan: input["plan"].as_str().unwrap_or("").to_owned(),
                reply,
            },
        );
        self.publish(id, deltas)?;
        self.persist(id, None).await
    }

    /// resolveInteraction 命中 ExitPlanMode 审批：模式变更、plan 文件与 ACK 一起提交后才释放 loop。
    pub(super) async fn resolve_plan_exit(
        &mut self,
        c: &Command,
        id: &str,
        interaction: &str,
    ) -> Result<Value> {
        let revision = self.sessions.get(id).map_or(0, |s| s.revision);
        let owned = self.plan_exits.get(interaction).is_some_and(|w| {
            w.session == id
                && self
                    .active
                    .get(id)
                    .is_some_and(|a| a.run_id == w.run && !a.cancel.is_cancelled())
        });
        if !owned {
            return Ok(c.ack("noop", revision, Some("proto.alreadyResolved")));
        }
        let waiting = self.plan_exits.remove(interaction).unwrap();
        let decision = plan_mode::decide(&c.payload["answer"]);
        if decision == Decision::Approve {
            // TS persistApprovedPlanFileBeforeExitPlanMode 同样吞掉非取消类写入失败：
            // plan 文件只是续作参考，不能因此阻止用户已批准的退出。
            if let Some(name) = plan_mode::plan_file_name(id) {
                let _ = self.tools.write_plan_file(&name, &waiting.plan).await;
            }
        }
        let s = self.sessions.get_mut(id).unwrap();
        let denied = ToolControl {
            denied: true,
            stop_turn: false,
        };
        let output = match decision {
            Decision::Approve => {
                s.plan_enabled = false;
                s.plan_followups.push(plan_mode::exit_reminder());
                ToolOutput::text(plan_mode::exit_result(&waiting.plan))
            }
            Decision::Feedback(text) => {
                s.plan_followups.push(json!({"role":"user","content":text}));
                failed(plan_mode::NOT_APPROVED.into(), denied)
            }
            Decision::Decline => failed(
                denied_content(None),
                ToolControl {
                    stop_turn: true,
                    ..denied
                },
            ),
        };
        s.pending.retain(|p| p["interactionId"] != interaction);
        let mut deltas = vec![];
        if let Some(row) = s
            .rows
            .iter_mut()
            .find(|r| r["toolCallId"] == waiting.call_id.as_str())
        {
            row["status"] = if output.control.denied {
                "cancelled"
            } else {
                "running"
            }
            .into();
            row.as_object_mut().unwrap().remove("approvalInteractionId");
            deltas.push(json!({"op":"row.upserted","row":row}));
        }
        let ack = self.commit_interaction(c, deltas).await?;
        let _ = waiting.reply.send(output);
        Ok(ack)
    }

    /// StepBoundary：plan 审批留下的消息先写入会话历史，再交给 loop 并入本地历史。
    pub(super) async fn drain_plan_followups(&mut self, id: &str) -> Result<Option<Vec<Value>>> {
        let s = self.sessions.get_mut(id).context("Session unavailable")?;
        if s.plan_followups.is_empty() {
            return Ok(None);
        }
        let messages = std::mem::take(&mut s.plan_followups);
        for message in &messages {
            s.append_message(message.clone());
        }
        self.persist(id, None).await?;
        Ok(Some(messages))
    }
}
