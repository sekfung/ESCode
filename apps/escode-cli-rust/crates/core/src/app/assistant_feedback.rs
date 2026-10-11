//! V4 `setAssistantFeedback`：助手回复点赞 / 点踩（TS `commands/handlers/assistant-feedback.ts`）。
//!
//! 修复：Rust 原先没有该命令，UI 点赞/点踩一律被 `guard.capabilityUnsupported` 拒绝并回滚。
//! TS 以 transcript metadata 为持久权威、事件推进投影；Rust 的行本身随会话落库，反馈直接写在
//! assistantText 行的 `feedback` 字段上（`null` 删除该字段），与命令 ACK 同一次提交，冷恢复随行还原。
use super::Engine;
use crate::domain::protocol::Command;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

impl Engine {
    pub(super) async fn assistant_feedback(&mut self, c: &Command) -> Result<Value> {
        let id = c.session_id.as_deref().context("Session id required")?;
        // 协议 COMMANDS_REQUIRING_BASE_REVISION：反馈是 CAS 命令（stale 已在 command 入口判定）。
        if c.base_revision.is_none() {
            bail!("baseRevision required for setAssistantFeedback");
        }
        let feedback = match &c.payload["feedback"] {
            Value::Null => None,
            Value::String(value) if matches!(value.as_str(), "like" | "dislike") => {
                Some(value.clone())
            }
            _ => bail!("Invalid assistant feedback"),
        };
        let target = &c.payload["target"];
        let s = self.sessions.get_mut(id).context("Session unavailable")?;
        let index = target
            .as_object()
            .filter(|t| t.len() == 2)
            .and_then(|_| {
                s.rows
                    .iter()
                    .position(|r| r["rowId"] == target["rowId"] && r["entityId"] == target["entityId"])
            });
        let Some(index) = index else {
            // TS 行目标解析不到时按过期目标处理（差分实测 stale / proto.staleTarget）。
            return Ok(c.ack("stale", s.revision, Some("proto.staleTarget")));
        };
        if s.rows[index]["kind"] != "assistantText" {
            return Ok(c.ack("rejected", s.revision, Some("guard.actionUnavailable")));
        }
        let row = &mut s.rows[index];
        let changed = match &feedback {
            Some(value) => row.get("feedback") != Some(&json!(value)),
            None => row.get("feedback").is_some(),
        };
        let mut deltas = vec![];
        if changed {
            match feedback {
                Some(value) => row["feedback"] = value.into(),
                None => {
                    row.as_object_mut().unwrap().remove("feedback");
                }
            }
            deltas.push(json!({"op":"row.upserted","row":row.clone()}));
            s.revision += 1;
        }
        // TS 对同值反馈仍返回 accepted（投影无变化），这里保持一致。
        let ack = c.ack("accepted", s.revision, None);
        self.publish(id, deltas)?;
        self.persist(id, Some((c.key(), ack.clone()))).await?;
        self.acks.insert(c.key(), ack.clone());
        Ok(ack)
    }
}
