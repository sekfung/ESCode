//! MCP server 主动通知 → live-only 会话 delta（docs/specs/rust-mcp-client-events.md S2）。
//!
//! 通知由连接层转成 `McpClientEvent`，订阅归属与凭证校验在工具层；这里只负责按会话广播，
//! 不推进会话 seq、不写 delta_log（与 TS `sequenceNumber: 0` 同义）。
use super::Engine;
use crate::contract::McpClientEvent;
use anyhow::Result;
use serde_json::json;

impl Engine {
    pub(super) async fn mcp_client_event(&mut self, event: McpClientEvent) -> Result<()> {
        if let McpClientEvent::Sampling { params, reply, .. } = event {
            let result = self.sample_from_server(&params).await;
            let _ = reply.send(result.map_err(|error| error.to_string()));
            return Ok(());
        }
        let deltas = self.tools.mcp_ui_notifications(&event).await?;
        for (session, delta) in deltas {
            self.publish_live(&session, delta)?;
        }
        Ok(())
    }

    /// 只发给当前在线订阅者：`from == to == session.seq`，不进入 delta_log/冷恢复。
    pub(super) fn publish_live(&mut self, id: &str, delta: serde_json::Value) -> Result<()> {
        let Some(session) = self.sessions.get(id) else {
            // 冷会话没有在线投影；通知丢弃（live-only）。
            return Ok(());
        };
        let at = session.seq;
        let topic = format!("conversation/{id}");
        let ids = self
            .subscriptions
            .values()
            .filter(|sub| sub.topic == topic && !sub.paused && !sub.needs_resync)
            .map(|sub| sub.id.clone())
            .collect::<Vec<_>>();
        for sub in ids {
            self.push_frame(
                &sub,
                "online",
                at,
                at,
                json!({ "kind": "deltas", "deltas": [delta.clone()] }),
            )?;
        }
        Ok(())
    }
}
