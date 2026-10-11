//! 回合与工具的用量事实（TS recordTurnUsageFact / turn-tool-usage → turn_usage / tool_usage）：
//! `v4/usage/stats` 的回合数、会话时长、工具调用与错误率来自这里。开始时刻取行的 createdAt
//! （turnHeader / toolCall 行在开始时创建），结束时刻取事件到达时。写入异步、失败不影响会话。
use super::{Engine, Event};
use serde_json::json;

impl Engine {
    pub(super) fn record_usage_facts(&self, id: &str, turn: &str, event: &Event) {
        let Some(s) = self.sessions.get(id) else {
            return;
        };
        let now = self.clock.now();
        let started = |kind: &str, key: &str, value: &str| {
            s.rows
                .iter()
                .rev()
                .find(|r| r["kind"] == kind && r[key] == value)
                .and_then(|r| r["createdAt"].as_u64())
                .unwrap_or(now)
        };
        let request = match event {
            Event::ToolDone {
                id: call,
                tool,
                failed,
                denied,
                ..
            } => json!({
                "op": "tool", "sessionId": id, "turnId": turn, "toolCallId": call, "toolName": tool,
                "status": if *denied { "cancelled" } else if *failed { "error" } else { "completed" },
                "startedAt": started("toolCall", "toolCallId", call), "completedAt": now,
            }),
            Event::Finished {
                error, cancelled, ..
            } => json!({
                "op": "turn", "sessionId": id, "turnId": turn,
                "status": if *cancelled { "cancelled" } else if error.is_some() { "error" } else { "completed" },
                "startedAt": started("turnHeader", "turnId", turn), "completedAt": now,
            }),
            _ => return,
        };
        let store = self.store.clone();
        tokio::spawn(async move {
            let _ = store.usage(request).await;
        });
    }
}

impl Engine {
    /// `v4/usage/stats`：range `all | 7d | 30d`（缺省 30 天），时区缺省 UTC。
    pub(super) async fn usage_stats(
        &self,
        params: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let range = params["range"].as_str().unwrap_or_default();
        anyhow::ensure!(
            matches!(range, "all" | "7d" | "30d"),
            "Invalid usage range: {range}"
        );
        let time_zone = params["timeZone"].as_str().unwrap_or("UTC");
        let until = self.clock.now() as i64;
        let tz_offset_ms = self.clock.tz_offset_ms(time_zone, until);
        let days = match range {
            "7d" => 7,
            _ => 30,
        };
        let since = if range == "all" {
            0
        } else {
            until - days * 86_400_000
        };
        let request =
            json!({ "op": "app", "since": since, "until": until, "tzOffsetMs": tz_offset_ms });
        let result = self.store.usage(request).await?;
        let opts = crate::domain::usage_stats::Options {
            range,
            time_zone,
            tz_offset_ms,
            generated_at: until,
            since,
            until,
        };
        Ok(crate::domain::usage_stats::snapshot(&result, &opts))
    }
}
