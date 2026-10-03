//! 工作流 run 的完成通知与 run 中通知（docs/specs/rust-dynamic-workflow.md M1 / M3）：工作流宿主经工具层 Host 通道报告结算，
//! 通知文本与 originMeta 由宿主用 TS 同一个格式器生成。会话空闲时作为一个后台结果轮注入（与后台子代理
//! 的续跑同一条路：`admit_input` → 行标 `backgroundResult` → 起跑），忙时留在会话里等下一次空闲。
use super::Engine;
use anyhow::Result;
use serde_json::{Value, json};

impl Engine {
    pub(super) async fn workflow_settled(&mut self, id: &str, notice: Value) -> Result<()> {
        if self.closed.contains(id) {
            return Ok(());
        }
        self.ensure_session(id).await?;
        let Some(session) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        // hooks 生命周期（docs/specs/rust-hooks.md）：投影成 hookInvocation 行。
        if notice["kind"] == "hookEvent" {
            return self.hook_event(id, &notice).await;
        }
        if notice["kind"] == "mailboxGuide" {
            return self.mailbox_guide(id, &notice).await;
        }
        if notice["kind"] == "permissionHook" {
            return self.permission_hook(id, &notice).await;
        }
        // 宿主派生的遥测事实（workflow.lifecycle）：Rust 重新盖基字段后发出。
        if notice["kind"] == "telemetry" {
            let mut fields = notice["fact"].clone();
            let kind = fields["kind"].as_str().unwrap_or_default().to_owned();
            if let Some(fields) = fields.as_object_mut() {
                fields.remove("kind");
            }
            self.emit_fact(id, None, &kind, fields);
            return Ok(());
        }
        // V4 `workflowRuns` 状态键：整键替换进会话快照，随下一次 state patch 发布。
        if notice["kind"] == "workflowRuns" {
            session.workflow_runs = Some(notice["workflowRuns"].clone());
            session.workflow_runs_legacy = Some(notice["legacy"].clone()).filter(|v| !v.is_null());
            session.revision += 1;
            let ops = notice["deltas"].as_array().cloned().unwrap_or_default();
            self.publish(id, ops)?;
            return self.persist(id, None).await;
        }
        // 同一条通知只排一次：结算按 run 去重，run 中通知（升级问答 / 停滞）另带 noticeId。
        let key = (notice["taskId"].clone(), notice["noticeId"].clone());
        if session
            .workflow_notices
            .iter()
            .any(|n| (n["taskId"].clone(), n["noticeId"].clone()) == key)
        {
            return Ok(());
        }
        session.workflow_notices.push(notice);
        self.persist(id, None).await?;
        self.deliver_workflow_notices(id).await
    }

    pub(super) async fn deliver_workflow_notices(&mut self, id: &str) -> Result<()> {
        let Some(s) = self.sessions.get(id) else {
            return Ok(());
        };
        // 设置轮未落完前不交付 run 通知：修订的设置轮必须先于新 run 的任何通知（docs/specs/rust-v4-command-gaps.md）。
        if s.running()
            || !s.auto_drain
            || !s.queue.is_empty()
            || !s.settings_turns.is_empty()
            || s.workflow_notices.is_empty()
        {
            return Ok(());
        }
        let notice = s.workflow_notices[0].clone();
        self.select(&json!({}), Some(self.session_selection(id)?))?;
        let text = crate::domain::background::task_notification_message(
            notice["text"].as_str().unwrap_or_default(),
        );
        let command = super::subagents::child_command(id, &self.clock.id(), &text);
        let (turn, _) = self.admit_input(id, &command, None, None)?;
        let s = self.sessions.get_mut(id).unwrap();
        s.workflow_notices.remove(0);
        for row in s.rows.iter_mut().filter(|r| r["turnId"] == turn) {
            if row["kind"] == "turnHeader" || row["kind"] == "userInput" {
                row["origin"] = "backgroundResult".into();
                if notice["originMeta"].is_object() {
                    row["originMeta"] = notice["originMeta"].clone();
                }
            }
        }
        self.publish(id, self.new_turn_rows(id))?;
        self.persist(id, None).await?;
        self.start_run(id, turn)
    }

    /// StepBoundary 的待并入消息：plan 审批的后续消息 → 子代理 mailbox → 后台子代理完成 → 后台工作流 / Bash 通知（各取其一类）。
    pub(super) async fn drain_step_messages(
        &mut self,
        id: &str,
        turn: &str,
    ) -> Result<Option<Vec<Value>>> {
        if let Some(messages) = self.drain_plan_followups(id).await? {
            return Ok(Some(messages));
        }
        if let Some(messages) = self.drain_mailbox(id, turn).await? {
            return Ok(Some(messages));
        }
        if let Some(messages) = self.steer_children(id, turn).await? {
            return Ok(Some(messages));
        }
        self.steer_workflow_notices(id, turn).await
    }

    /// TS drainPendingRuntimeCommandsForActiveLoop：回合进行中到达的工作流通知在下一个步边界并入本回合
    /// （task_notification_steer），而不是等回合结束再开一个后台结果轮。
    async fn steer_workflow_notices(&mut self, id: &str, turn: &str) -> Result<Option<Vec<Value>>> {
        let s = self.sessions.get_mut(id).unwrap();
        let (steered, kept): (Vec<Value>, Vec<Value>) =
            std::mem::take(&mut s.workflow_notices).into_iter().partition(|n| n["steer"] != false);
        s.workflow_notices = kept;
        if steered.is_empty() {
            return Ok(None);
        }
        let mut deltas = vec![];
        let mut messages = vec![];
        for notice in steered {
            let text = crate::domain::background::task_notification_message(
                notice["text"].as_str().unwrap_or_default(),
            );
            let mut row = s.row("userInput", turn, &self.clock.id(), self.clock.now());
            row["text"] = text.clone().into();
            row["origin"] = "backgroundResult".into();
            if notice["originMeta"].is_object() {
                row["originMeta"] = notice["originMeta"].clone();
            }
            s.rows.push(row.clone());
            deltas.push(json!({"op":"row.appended","row":row}));
            let message = json!({"role":"user","content":text});
            s.append_message(message.clone());
            messages.push(message);
        }
        s.revision += 1;
        self.publish(id, deltas)?;
        self.persist(id, None).await?;
        Ok(Some(messages))
    }
}

/// 每订阅者的 `workflowRuns` 编码（TS publisher）。日志里存原生 op 与整键 patch 两份事实：
/// - 有 `workflowRunDeltas` 能力：收 `workflowRun.*` 键级增量，state patch 里不再带整键；
/// - 旧消费者：丢掉 `workflowRun.*`（它们的判别式旧客户端不认），快照与 patch 里的整键换成当前旧界裁剪版。
pub(super) fn encode_workflow_runs(payload: &mut Value, deltas_capable: bool, legacy: Option<&Value>) {
    if !deltas_capable
        && let Some(legacy) = legacy
        && payload["snapshot"].get("workflowRuns").is_some()
    {
        payload["snapshot"]["workflowRuns"] = legacy.clone();
    }
    let Some(deltas) = payload["deltas"].as_array_mut() else {
        return;
    };
    if !deltas_capable {
        deltas.retain(|d| !d["op"].as_str().is_some_and(|op| op.starts_with("workflowRun.")));
    }
    for delta in deltas.iter_mut() {
        let Some(patch) = delta["patch"].as_object_mut() else {
            continue;
        };
        if deltas_capable {
            patch.remove("workflowRuns");
        } else if let Some(legacy) = legacy
            && patch.contains_key("workflowRuns")
        {
            patch.insert("workflowRuns".into(), legacy.clone());
        }
    }
}
