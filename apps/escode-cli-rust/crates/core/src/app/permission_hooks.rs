//! PermissionRequest hooks（TS permission-flow `racePermissionResponders` + hook-flow `runPermissionRequestHooks`，
//! docs/specs/rust-hooks.md H1）：确认窗挂起的同时跑 hook 链，与用户应答竞速，先到的决定生效；hook 没有结论
//! （或失败）时退赛，确认窗继续等用户。
use super::Engine;
use crate::contract::{Event, EventSink, PermissionOutcome};
use anyhow::Result;
use serde_json::{Value, json};

/// TS runPermissionRequestHooks 的结论映射：None 表示 hook 退赛。`modify`（改写入参）需要按新入参重新判定
/// 权限，暂未支持，按退赛处理（确认窗继续等用户）。
fn outcome(result: &Value) -> Option<PermissionOutcome> {
    const DENIED: &str = "Denied by PermissionRequest hook";
    let deny = |reason: Option<&str>| {
        let reason = reason.unwrap_or(DENIED);
        Some(PermissionOutcome::deny(
            crate::domain::tool_failure::plain_error_text(reason),
        ))
    };
    if result["preventContinuation"] == true {
        return deny(result["stopReason"].as_str());
    }
    let decision = &result["permissionRequestResult"];
    if decision.is_null() {
        return match result["permissionBehavior"].as_str() {
            Some("deny") => deny(result["stopReason"].as_str()),
            Some("allow") => Some(PermissionOutcome::allow()),
            _ => None,
        };
    }
    match decision["behavior"].as_str() {
        Some("deny") => deny(decision["message"].as_str()),
        _ if decision.get("updatedInput").is_some_and(|v| !v.is_null()) => None,
        _ => Some(PermissionOutcome::allow()),
    }
}

impl Engine {
    /// 确认窗挂起后启动 hook 链；结论经 Host 通道回到 owner（`permission_hook`）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_permission_hook(
        &self,
        id: &str,
        turn: &str,
        call: &Value,
        input: &Value,
        interaction: &str,
        reason: &str,
    ) {
        let Some(s) = self.sessions.get(id) else {
            return;
        };
        let hook_input = json!({
            "hookEventName": "PermissionRequest", "mode": s.mode, "reason": reason, "requestId": interaction,
            "sessionId": id, "toolCallId": call["id"], "toolInput": input, "toolName": call["function"]["name"],
            "traceId": s.trace_id, "turnId": turn,
        });
        let sink = EventSink {
            session_id: crate::contract::HOST_CHANNEL.into(),
            run_id: crate::contract::HOST_CHANNEL.into(),
            tx: self.events.clone(),
        };
        let (tools, session, interaction) =
            (self.tools.clone(), id.to_owned(), interaction.to_owned());
        let call_id = call["id"].as_str().map(str::to_owned);
        tokio::spawn(async move {
            let Some(result) = tools
                .run_hook(&session, hook_input, call_id.as_deref())
                .await
            else {
                return;
            };
            let notice =
                json!({"kind": "permissionHook", "interaction": interaction, "result": result});
            let _ = sink.send(Event::WorkflowSettled { session, notice }).await;
        });
    }

    /// hook 链的结论：确认仍挂起时按它收口（与用户应答同一套行与遥测更新），否则忽略。
    pub(super) async fn permission_hook(&mut self, id: &str, notice: &Value) -> Result<()> {
        let interaction = notice["interaction"].as_str().unwrap_or_default();
        let Some(outcome) = outcome(&notice["result"]) else {
            return Ok(());
        };
        let Some(waiting) = self
            .waiting_permissions
            .get(interaction)
            .filter(|w| w.session == id)
        else {
            return Ok(());
        };
        let owned = self
            .active
            .get(id)
            .is_some_and(|a| a.run_id == waiting.run && !a.cancel.is_cancelled());
        if !owned {
            return Ok(());
        }
        let waiting = self.waiting_permissions.remove(interaction).unwrap();
        let Some(s) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        s.pending.retain(|p| p["interactionId"] != interaction);
        let mut deltas = vec![];
        if let Some(row) = s
            .rows
            .iter_mut()
            .find(|r| r["toolCallId"] == waiting.call_id.as_str())
        {
            row["status"] = if outcome.allowed {
                "running"
            } else {
                "cancelled"
            }
            .into();
            row.as_object_mut().unwrap().remove("approvalInteractionId");
            deltas.push(json!({"op":"row.upserted","row":row.clone()}));
        }
        s.revision += 1;
        s.updated_at = self.clock.now();
        self.activate_question_head(id);
        self.publish(id, deltas)?;
        self.persist(id, None).await?;
        let turn = self.active.get(id).map(|a| a.turn_id.clone());
        let decision = if outcome.allowed { "allow" } else { "deny" };
        let p = super::telemetry::Permission {
            phase: "resolved",
            call_id: &waiting.call_id,
            tool: None,
            request_id: Some(interaction),
            decision: Some(decision),
        };
        self.telemetry_permission(id, turn.as_deref(), p);
        let _ = waiting.reply.send(outcome);
        Ok(())
    }
}
