use super::Engine;
use anyhow::Result;
impl Engine {
    pub(super) async fn background_event(
        &mut self,
        id: &str,
        run: &str,
        task: crate::domain::background::BackgroundTask,
        committed: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<()> {
        let Some(session) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        let known = session.background.get(&task.id);
        let valid = if task.status == "running" {
            known.is_none()
                && self
                    .active
                    .get(id)
                    .is_some_and(|a| a.run_id == run && !a.cancel.is_cancelled())
        } else {
            known.is_some_and(|prior| prior.run_id == run && prior.status == "running")
        };
        if !valid || task.run_id != run {
            return Ok(());
        }
        if session.background.len() >= 128
            && known.is_none()
            && let Some(old) = session
                .background
                .values()
                .filter(|t| t.status != "running")
                .min_by_key(|t| t.started_at)
                .map(|t| t.id.clone())
        {
            session.background.remove(&old);
        }
        // TS BackgroundTaskTracker：local_bash 终态发一条完成通知（空闲时开后台结果轮，忙时在步边界并入本回合）。
        // 已结束的子代理会话不再收（TS shouldSuppressSealedSubagentBashNotification）。
        let sealed = session.parent_id.is_some() && !session.running();
        if task.status != "running" && !sealed {
            let (text, meta) = task.notification();
            // 被停止的任务（TaskStop / 取消）：停止的工具结果先交回模型，通知在本轮结束后作为后台结果轮送达，
            // 不在步边界并入（Node 的 tracker 轮询晚于停止那一步的请求）。
            let stopped = matches!(task.status.as_str(), "cancelled" | "killed" | "stopped");
            session.workflow_notices.push(serde_json::json!({
                "taskId": task.id, "noticeId": "settled", "text": text, "originMeta": meta,
                "steer": !stopped,
            }));
        }
        session.background.insert(task.id.clone(), task);
        session.updated_at = self.clock.now();
        session.revision += 1;
        self.publish(id, vec![])?;
        self.persist(id, None).await?;
        if let Some(receipt) = committed {
            let _ = receipt.send(());
        }
        self.resume_background_goal(id).await?;
        self.deliver_workflow_notices(id).await?;
        self.finish_child(id).await?;
        Ok(())
    }
}
