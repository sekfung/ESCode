//! 权限判定与确认：对应 TS `tool/executor/permission-flow.ts`。
//! 判定在会话 owner 内完成；ask 时挂起工具调用并投影 pendingInteraction，等待 resolveInteraction。
use super::Engine;
use crate::contract::PermissionOutcome;
use crate::domain::permission::Rule;
use crate::domain::permission::{Capability, Context as PermissionContext, Mode, Ruleset, check};
use crate::domain::permission_options::{default_updates, denied_content, options};
use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::oneshot;

pub(super) struct WaitingPermission {
    pub session: String,
    pub run: String,
    pub call_id: String,
    /// 「总是允许」要写入的项目规则（allowAlways 应答时落库）。
    pub suggested: Vec<Rule>,
    pub reply: oneshot::Sender<PermissionOutcome>,
}

impl Engine {
    /// 工具调用前的权限判定；allow/deny 立即答复，ask 则挂起等待用户应答。
    pub(super) async fn ask_permission(
        &mut self,
        id: &str,
        run: &str,
        turn: &str,
        call: &Value,
        reply: oneshot::Sender<PermissionOutcome>,
    ) -> Result<()> {
        let tool = call["function"]["name"].as_str().unwrap_or("").to_owned();
        let input: Value = call["function"]["arguments"]
            .as_str()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(Value::Null);
        let Some(session) = self.sessions.get(id) else {
            let _ = reply.send(PermissionOutcome::deny(denied_content(None)));
            return Ok(());
        };
        let ctx = PermissionContext {
            tool_name: &tool,
            mode: Mode::parse(&session.mode).unwrap_or(Mode::Build),
            plan_enabled: Some(session.plan_enabled),
            input: &input,
            project: self.project_rules.as_ref(),
            session: None,
        };
        let capability: Option<Capability> = self.tools.permission_capability(&tool, &input);
        let decision = check(&ctx, capability.as_ref());
        // 待产品确认：TS 对声明 requiresUserInteraction 的工具（AskUserQuestion）在
        // checkPermission 里同样返回 ask，而该工具自身的提问界面才是这次「用户交互」。
        // Rust 现有问句流程与既有集成用例都按「不额外弹权限确认」实现，这里先按原行为放行，
        // 不与 TS 判定层冲突（permission::check 保持逐位对齐），差异记录在待办。
        if decision.rule_id == "tool.userInteraction" {
            let _ = reply.send(PermissionOutcome::allow());
            return Ok(());
        }
        match decision.behavior {
            crate::domain::permission::Behavior::Allow => {
                let _ = reply.send(PermissionOutcome::allow());
                Ok(())
            }
            crate::domain::permission::Behavior::Deny => {
                let _ = reply.send(PermissionOutcome::deny(denied_content(None)));
                Ok(())
            }
            crate::domain::permission::Behavior::Ask => {
                self.register_permission(id, run, turn, call, &tool, &input, reply)
                    .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn register_permission(
        &mut self,
        id: &str,
        run: &str,
        turn: &str,
        call: &Value,
        tool: &str,
        input: &Value,
        reply: oneshot::Sender<PermissionOutcome>,
    ) -> Result<()> {
        if reply.is_closed() {
            return Ok(());
        }
        let suggested = suggested_rules(tool, input);
        // 项目规则可由 store 持久化时才投放 allowAlways，避免给出无法兑现的授权。
        let persistent = self.project_rules_persistent;
        let interaction = self.clock.id();
        let now = self.clock.now();
        let call_id = call["id"].as_str().unwrap_or("").to_owned();
        let Some(s) = self.sessions.get_mut(id) else {
            let _ = reply.send(PermissionOutcome::deny(denied_content(None)));
            return Ok(());
        };
        if let Some(row) = s
            .rows
            .iter_mut()
            .find(|r| r["turnId"] == turn && r["toolCallId"] == call["id"])
        {
            row["status"] = "pendingApproval".into();
            row["approvalInteractionId"] = interaction.clone().into();
        }
        s.pending.push(json!({
            "interactionId": interaction,
            "kind": "permission",
            "anchorRowId": s.rows.iter().find(|r| r["toolCallId"] == call["id"]).map(|r| r["rowId"].clone()).unwrap_or(Value::Null),
            "createdAt": now,
            "payload": {
                "kind": "permission",
                "toolCallId": call["id"],
                "toolName": tool,
                "summary": format!("Allow {tool}?"),
                "detail": input,
                "freeText": true,
                "options": options_with_persistence(&suggested, persistent),
            }
        }));
        s.revision += 1;
        s.updated_at = now;
        self.waiting_permissions.insert(
            interaction.clone(),
            WaitingPermission {
                session: id.to_owned(),
                run: run.to_owned(),
                call_id,
                suggested,
                reply,
            },
        );
        let row = self.sessions[id]
            .rows
            .iter()
            .find(|r| r["toolCallId"] == call["id"])
            .cloned();
        let mut deltas = vec![];
        if let Some(row) = row {
            deltas.push(json!({"op":"row.upserted","row":row}));
        }
        self.publish(id, deltas)?;
        self.persist(id, None).await
    }
}

fn options_with_persistence(suggested: &[Rule], persistent: bool) -> Vec<Value> {
    let mut list = options(suggested, !persistent);
    if persistent {
        return list;
    }
    // 无法持久化项目规则时不投放 allowAlways；会话授权的语义见 spec。
    list.retain(|o| o["optionId"] != "allowAlways");
    list
}

/// 「总是允许」的规则：Bash 用稳定前缀建议，其余工具按输入键取内容。
pub(super) fn suggested_rules(tool: &str, input: &Value) -> Vec<Rule> {
    if tool == "Bash"
        && let Some(command) = input["command"].as_str()
    {
        let rules: Vec<Rule> = crate::domain::bash_rule_policy::BashRulePolicy::new(command)
            .suggestions
            .into_iter()
            .map(|content| Rule {
                tool_name: tool.to_owned(),
                rule_content: Some(content),
            })
            .collect();
        if !rules.is_empty() {
            return rules;
        }
    }
    default_updates(tool, input)
}

impl Engine {
    /// 「总是允许」：把建议规则并入项目规则集并落库；失败时拒绝这次授权而不是静默放行。
    pub(super) async fn merge_project_rules(&mut self, rules: &[Rule]) -> bool {
        if !self.project_rules_persistent {
            return false;
        }
        let mut json = self
            .project_rules_json
            .clone()
            .unwrap_or_else(|| json!({"version": 1}));
        let mut merged = json["allow"].as_array().cloned().unwrap_or_default();
        for rule in rules {
            let entry = match &rule.rule_content {
                Some(content) => json!({"toolName": rule.tool_name, "ruleContent": content}),
                None => json!({"toolName": rule.tool_name}),
            };
            let duplicate = merged.iter().any(|existing| {
                existing["toolName"] == entry["toolName"]
                    && existing["ruleContent"] == entry["ruleContent"]
            });
            if !duplicate {
                merged.push(entry);
            }
        }
        json["allow"] = Value::Array(merged);
        if self
            .store
            .save_project_rules(&self.workspace, &json)
            .await
            .is_err()
        {
            return false;
        }
        self.project_rules = Some(Ruleset::from_json(&json));
        self.project_rules_json = Some(json);
        true
    }
}
