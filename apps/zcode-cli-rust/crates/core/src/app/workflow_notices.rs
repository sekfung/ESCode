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
        if s.running() || !s.auto_drain || !s.queue.is_empty() || s.workflow_notices.is_empty() {
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
}
