use super::Engine;
use crate::domain::{
    protocol::Command,
    question::{QuestionAnswer, QuestionInput},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::oneshot;

pub(super) struct WaitingQuestion {
    pub session: String,
    pub run: String,
    pub eligible: bool,
    pub reply: oneshot::Sender<QuestionAnswer>,
}
impl Engine {
    pub(super) async fn register_question(
        &mut self,
        id: &str,
        run: &str,
        call: &str,
        input: QuestionInput,
        reply: oneshot::Sender<QuestionAnswer>,
    ) -> Result<()> {
        if reply.is_closed() {
            return Ok(());
        }
        let now = self.clock.now();
        let interaction = self.clock.id();
        let s = self.sessions.get_mut(id).context("Session unavailable")?;
        let row = s
            .rows
            .iter_mut()
            .find(|r| r["toolCallId"] == call)
            .context("Question tool row missing")?;
        row["inputText"] = serde_json::to_string(&input)?.into();
        row["status"] = "pendingApproval".into();
        row["approvalInteractionId"] = interaction.clone().into();
        let delta = json!({"op":"row.upserted","row":row});
        s.pending.push(json!({"interactionId":interaction,"kind":"userInput","anchorRowId":row["rowId"],"createdAt":now,"payload":input.payload(call)}));
        s.revision += 1;
        s.updated_at = now;
        self.questions.insert(
            interaction,
            WaitingQuestion {
                session: id.into(),
                run: run.into(),
                eligible: self.auto_resolution_preference,
                reply,
            },
        );
        self.activate_question_head(id);
        self.publish(id, vec![delta])?;
        self.persist(id, None).await
    }
    pub(super) async fn interaction_command(&mut self, c: &Command) -> Result<Value> {
        let id = c.session_id.as_deref().context("Session id required")?;
        let interaction = c.payload["interactionId"]
            .as_str()
            .context("Interaction id required")?;
        let revision = self.sessions.get(id).map_or(0, |s| s.revision);
        let owned = self.questions.get(interaction).is_some_and(|q| {
            q.session == id
                && self
                    .active
                    .get(id)
                    .is_some_and(|a| a.run_id == q.run && !a.cancel.is_cancelled())
        });
        if c.kind == "snoozeInteractionAutoResolution" {
            if !owned || !self.snooze_question(id, interaction) {
                return Ok(c.ack("noop", revision, Some("proto.alreadyResolved")));
            }
            return self.commit_interaction(c, vec![]).await;
        }
        if owned {
            let input = self.sessions[id]
                .pending
                .iter()
                .find(|p| p["interactionId"] == interaction)
                .context("Question missing")?["payload"]["input"]
                .clone();
            let answer = QuestionInput::parse(input)?.answer(c.payload["answer"].clone())?;
            let deltas = self.settle_question(id, interaction, &answer)?;
            let ack = self.commit_interaction(c, deltas).await?;
            // 答案与 ACK 提交成功后才释放 waiter；事务失败不会产生下一个工具结果或模型请求。
            let q = self.questions.remove(interaction).unwrap();
            let _ = q.reply.send(answer);
            return Ok(ack);
        }
        if let Some(waiting) = self.waiting_permissions.get(interaction)
            && waiting.session == id
        {
            // 运行已结束（stop/取消）时不再放行：迟到的应答按已解决处理，工具不会执行。
            let owned = self
                .active
                .get(id)
                .is_some_and(|a| a.run_id == waiting.run && !a.cancel.is_cancelled());
            if !owned {
                return Ok(c.ack("noop", revision, Some("proto.alreadyResolved")));
            }
            let call_id = waiting.call_id.clone();
            let option = c.payload["answer"]["optionId"]
                .as_str()
                .context("Permission option required")?;
            let feedback = c.payload["answer"]["freeText"].as_str();
            let waiting_parent = self.sessions[id].parent_id.clone();
            let outcome = match option {
                "allowOnce" | "allowSession" => crate::contract::PermissionOutcome::allow(),
                "allowAlways" => {
                    let rules = waiting.suggested.clone();
                    let merged = self.merge_project_rules(&rules).await;
                    if merged {
                        crate::contract::PermissionOutcome::allow()
                    } else {
                        crate::contract::PermissionOutcome::deny(
                            crate::domain::permission_options::denied_content(None),
                        )
                    }
                }
                // 完全访问：会话与已接纳的排队输入一起切到 yolo，再放行本次调用（TS
                // commitPermissionFullAccess）。模式变更随下方 commit_interaction 与 ACK 同一次提交。
                super::permission_flow::FULL_ACCESS_OPTION_ID if waiting_parent.is_none() => {
                    let s = self.sessions.get_mut(id).unwrap();
                    s.mode = "yolo".into();
                    for item in s.queue.iter_mut() {
                        if item.get("mode").is_some_and(|m| !m.is_null()) {
                            item["mode"] = "yolo".into();
                        }
                    }
                    crate::contract::PermissionOutcome::allow()
                }
                "deny" => crate::contract::PermissionOutcome::deny(
                    crate::domain::permission_options::denied_content(feedback),
                ),
                other => anyhow::bail!("Unsupported permission option: {other}"),
            };
            let s = self.sessions.get_mut(id).unwrap();
            s.pending.retain(|p| p["interactionId"] != interaction);
            // 行回到执行态：工具即将执行，最终状态由 ToolDone 落定；拒绝同样经 ToolDone。
            if let Some(row) = s
                .rows
                .iter_mut()
                .find(|r| r["toolCallId"] == call_id.as_str())
            {
                // TS settlePermission：拒绝立即收口为 cancelled，其余回到 running。
                row["status"] = if outcome.allowed {
                    "running"
                } else {
                    "cancelled"
                }
                .into();
                row.as_object_mut().unwrap().remove("approvalInteractionId");
            }
            self.activate_question_head(id);
            let ack = self.commit_interaction(c, vec![]).await?;
            let waiting = self.waiting_permissions.remove(interaction).unwrap();
            let _ = waiting.reply.send(outcome);
            return Ok(ack);
        }
        Ok(c.ack("noop", revision, Some("proto.alreadyResolved")))
    }
    async fn commit_interaction(&mut self, c: &Command, deltas: Vec<Value>) -> Result<Value> {
        let id = c.session_id.as_deref().unwrap();
        let s = self.sessions.get_mut(id).unwrap();
        s.revision += 1;
        s.updated_at = self.clock.now();
        let mut ack = c.ack("accepted", s.revision, None);
        if c.kind == "resolveInteraction" {
            ack["result"] =
                json!({"type":"resolveInteraction","resolvedBy":{"clientId":c.client_id}});
            if let Some(option) = c.payload["answer"]["optionId"].as_str() {
                ack["result"]["resolvedBy"]["optionId"] = option.into();
            }
        }
        self.publish(id, deltas)?;
        self.persist(id, Some((c.key(), ack.clone()))).await?;
        self.acks.insert(c.key(), ack.clone());
        Ok(ack)
    }
    pub(super) fn settle_question(
        &mut self,
        id: &str,
        interaction: &str,
        answer: &QuestionAnswer,
    ) -> Result<Vec<Value>> {
        let s = self.sessions.get_mut(id).context("Session unavailable")?;
        let pending = s
            .pending
            .iter()
            .find(|p| p["interactionId"] == interaction)
            .context("Question unavailable")?;
        let call = pending["payload"]["toolCallId"].as_str().unwrap();
        let row = s
            .rows
            .iter_mut()
            .find(|r| r["toolCallId"] == call)
            .context("Question row unavailable")?;
        // 用户答案先成为已提交工具行事实；崩溃恢复可从此补齐尚未来得及提交的 canonical result。
        if !answer.failed {
            let mut input: Value = serde_json::from_str(row["inputText"].as_str().unwrap())?;
            input.as_object_mut().unwrap().remove("answers");
            input.as_object_mut().unwrap().remove("annotations");
            for (key, value) in answer.data.as_object().unwrap() {
                input[key] = value.clone();
            }
            row["inputText"] = serde_json::to_string(&input)?.into();
        }
        row["status"] = if answer.failed { "error" } else { "success" }.into();
        row["output"] = json!({"text":answer.content});
        row["endedAt"] = self.clock.now().into();
        if answer.failed {
            row["error"] = json!({"code":"tool_execution_failed","message":answer.content});
        }
        row.as_object_mut().unwrap().remove("approvalInteractionId");
        let deltas = vec![json!({"op":"row.upserted","row":row})];
        s.pending.retain(|p| p["interactionId"] != interaction);
        self.activate_question_head(id);
        Ok(deltas)
    }
}
