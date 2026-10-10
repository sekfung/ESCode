//! 工作区（项目）hooks 的信任准入与审核（docs/specs/rust-hooks.md H2）：准入 / 审核由工作流宿主里的 TS
//! `createWorkspaceHookRuntimeSecurity` 裁决；这里把它的会话事件投影成 V4 `workspaceHookAdmission` 状态与
//! `workspaceHookReview` 待处理交互（TS product-projection），并把审核命令与无会话授权转给宿主。
use super::Engine;
use crate::domain::protocol::Command;
use anyhow::Result;
use serde_json::{Value, json};

impl Engine {
    /// 宿主转发的 `WorkspaceHook*` 会话事件。
    pub(super) async fn workspace_hook_event(&mut self, id: &str, event: &Value) -> Result<()> {
        let payload = &event["payload"];
        let Some(s) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        match event["type"].as_str().unwrap_or_default() {
            // 软门禁：pendingCount > 0 写入提示条状态，0 置 null。
            "workspace_hook_admission_updated" => {
                s.workspace_hook_admission = Some(if payload["pendingCount"].as_u64() == Some(0) {
                    Value::Null
                } else {
                    let mut state = json!({"pendingCount": payload["pendingCount"], "bundleDigest": payload["bundleDigest"]});
                    if let Some(identity) = payload["workspaceIdentity"].as_str() {
                        state["workspaceIdentity"] = identity.into();
                    }
                    state
                });
            }
            "workspace_hook_review_requested" => {
                let request = &payload["request"];
                let current = s
                    .pending
                    .iter()
                    .find(|p| p["kind"] == "workspaceHookReview");
                // TS verdictWorkspaceHookReviewRequest：同一 flow 的更高 generation 才能替换当前审核。
                if let Some(current) = current {
                    let current = &current["payload"];
                    let advance = current["reviewFlowId"] == request["reviewFlowId"]
                        && request["generation"].as_u64() > current["generation"].as_u64();
                    if !advance {
                        return Ok(());
                    }
                }
                s.pending.retain(|p| p["kind"] != "workspaceHookReview");
                s.pending.push(json!({
                    "interactionId": request["interactionId"], "kind": "workspaceHookReview",
                    "anchorRowId": null, "createdAt": request["createdAt"], "payload": request,
                }));
            }
            "workspace_hook_review_settled" | "workspace_hook_review_superseded" => {
                let before = s.pending.len();
                s.pending.retain(|p| {
                    !(p["kind"] == "workspaceHookReview"
                        && p["interactionId"] == payload["interactionId"])
                });
                if s.pending.len() == before {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }
        s.revision += 1;
        self.publish(id, vec![])?;
        self.persist(id, None).await
    }

    /// V4 审核命令（respond / toggle / revoke / request）：宿主拒绝时以 reasonCode 回 rejected。
    pub(super) async fn workspace_hook_command(&mut self, c: &Command, id: &str) -> Result<Value> {
        let revision = self.sessions.get(id).map_or(0, |s| s.revision);
        // TS requireWorkspaceHookReviewRecord：信封会话与载荷会话必须一致。
        if c.payload["sessionId"].as_str() != Some(id) {
            return Ok(c.ack(
                "rejected",
                revision,
                Some("workspace_hooks_snapshot_mismatch"),
            ));
        }
        let params = json!({
            "session": id, "cwd": self.workspace_path, "command": c.kind, "payload": c.payload,
        });
        let result = self
            .tools
            .workspace_hooks("hooks.review", params)
            .await
            .unwrap_or_else(|_| json!({"accepted": false, "reasonCode": "workspace_hooks_require_trust_capable_host"}));
        if result["accepted"] != true {
            let reason = result["reasonCode"]
                .as_str()
                .unwrap_or("workspace_hooks_require_trust_capable_host");
            return Ok(c.ack("rejected", revision, Some(reason)));
        }
        let ack = c.ack("accepted", revision, None);
        self.acks.insert(c.key(), ack.clone());
        Ok(ack)
    }

    /// `workspace/hooks/trustGrant`：Settings 无会话授权（宿主按 TS 口径重新发现并写信任库，再重载同工作区会话）。
    pub(super) async fn workspace_hook_trust_grant(&mut self, params: &Value) -> Result<Value> {
        self.tools
            .workspace_hooks("hooks.trustGrant", json!({ "params": params }))
            .await
    }
}

impl Engine {
    /// H3：mailbox PostToolUse 取到的消息作为本轮 guide 输入（TS steerTurn delivery guide + expectedTurnId）；
    /// 轮次已变或已空闲时丢弃（TS 记 queue_rejected）。
    pub(super) async fn mailbox_guide(&mut self, id: &str, notice: &Value) -> Result<()> {
        let current = self.active.get(id).map(|a| a.turn_id.clone());
        let expected = notice["turnId"].as_str();
        if current.is_none() || expected.is_some_and(|t| Some(t) != current.as_deref()) {
            return Ok(());
        }
        let c = Command {
            command_id: self.clock.id(),
            client_id: "session-mailbox".into(),
            session_id: Some(id.into()),
            kind: "sendText".into(),
            payload: json!({"text": notice["text"], "requestedDelivery": "guide"}),
            issued_at: self.clock.now() as f64,
            base_revision: None,
            base_log_epoch: None,
        };
        self.send_input(c).await.map(|_| ())
    }
}
