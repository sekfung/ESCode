//! 权限判定与确认：对应 TS `tool/executor/permission-flow.ts`。
//! 判定在会话 owner 内完成；ask 时挂起工具调用并投影 pendingInteraction，等待 resolveInteraction。
use super::Engine;
use crate::contract::PermissionOutcome;
use crate::domain::permission::Rule;
use crate::domain::permission::{Capability, Context as PermissionContext, Mode, Ruleset, check};
use crate::domain::permission_options::{ask_reason, denied_content};
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

/// Node headless broker 自动放行的工具（无交互审批面时工作流仍可创建 / 修订）。
const HEADLESS_AUTO_APPROVED: [&str; 2] = ["CreateWorkflow", "AmendWorkflow"];

impl Engine {
    /// `-p`：需要审批的工具调用直接拒绝（Node deny broker 文案），工作流创建 / 修订放行。
    pub fn with_headless_permissions(mut self) -> Self {
        self.headless_permissions = true;
        self
    }

    /// 工具调用前的权限判定；allow/deny 立即答复，ask 则挂起等待用户应答。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn ask_permission(
        &mut self,
        id: &str,
        run: &str,
        turn: &str,
        call: &Value,
        memory_root: Option<&str>,
        (approval_proceed, hook): (bool, Option<Value>),
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
        // 群任务的会话级授权代替项目规则（TS loadProjectPermissionRuleset 的 scope=session 分支）。
        let scoped = session
            .scoped_permission
            .as_ref()
            .map(crate::domain::permission::Ruleset::from_json);
        let ctx = PermissionContext {
            tool_name: &tool,
            mode: Mode::parse(&session.mode).unwrap_or(Mode::Build),
            plan_enabled: Some(session.plan_enabled),
            input: &input,
            project: scoped.as_ref().or(self.project_rules.as_ref()),
            session: None,
            config: Some(&self.permission_config),
        };
        // Wiki 工具只存在于 Rust runtime，能力表在 domain::wiki（WikiManage 为 alwaysAsk）。
        let mut capability: Option<Capability> = crate::domain::wiki::tools::capability(&tool)
            .or_else(|| self.tools.permission_capability(&tool, &input));
        // guarded（v3.15.1）：Bash 命中危险命令时附带 userApprovalRule，由 check 走 user-once 确认。
        if ctx.mode == Mode::Guarded && tool == "Bash" {
            let owner = self.shell_owner_session(id);
            let shell = self
                .shell
                .cached(&owner, "user-execution")
                .and_then(|value| value.get("integratedTerminalShell"))
                .filter(|value| !value.is_null());
            if let Some(rule) = self.tools.guarded_rule(&input, shell) {
                capability
                    .get_or_insert_with(Capability::default)
                    .user_approval_rule = Some(rule);
            }
        }
        // 主会话记忆 Markdown 写入放行（docs/specs/rust-project-memory.md）。
        let decision = crate::domain::memory::permission_override(
            check(&ctx, capability.as_ref()),
            &tool,
            &input,
            memory_root,
            &self.workspace_path,
        );
        let always_ask = capability.as_ref().and_then(|c| c.always_ask) == Some(true);
        // TS：user-once（guarded 危险命令）不经 PreToolUse 改判，也不被工具预览的 proceed 放行。
        let user_once = decision.user_once;
        let (decision, hook_reason) = if user_once {
            (decision, None)
        } else {
            super::tool_hooks::merge_permission(decision, hook.as_ref(), always_ask)
        };
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
                let reason =
                    crate::domain::permission_options::deny_reason(decision.rule_id, &tool);
                let call_id = call["id"].as_str().unwrap_or_default();
                let p = super::telemetry::Permission {
                    phase: "denied",
                    call_id,
                    tool: Some(&tool),
                    request_id: None,
                    decision: None,
                };
                self.telemetry_permission(id, Some(turn), p);
                let _ = reply.send(PermissionOutcome::deny(reason));
                Ok(())
            }
            // TS approval-gate：工具答复 proceed 时 ask 直接放行（只能收窄 ask，不能把 deny 变成 allow）。
            crate::domain::permission::Behavior::Ask if approval_proceed && !user_once => {
                let _ = reply.send(PermissionOutcome::allow());
                Ok(())
            }
            // `-p` 没有审批面：Node createHeadlessPermissionBroker 只放行工作流创建 / 修订，其余按 deny broker 拒绝。
            crate::domain::permission::Behavior::Ask if self.headless_permissions => {
                if HEADLESS_AUTO_APPROVED.contains(&tool.as_str()) {
                    let _ = reply.send(PermissionOutcome::allow());
                } else {
                    let call_id = call["id"].as_str().unwrap_or_default();
                    let p = super::telemetry::Permission {
                        phase: "denied",
                        call_id,
                        tool: Some(&tool),
                        request_id: None,
                        decision: None,
                    };
                    self.telemetry_permission(id, Some(turn), p);
                    let _ = reply.send(PermissionOutcome::deny(format!(
                        "No permission client configured for {tool}"
                    )));
                }
                Ok(())
            }
            crate::domain::permission::Behavior::Ask => {
                let reason = hook_reason.unwrap_or_else(|| ask_reason(decision.rule_id, &tool));
                self.register_permission(
                    id, run, turn, call, &tool, &input, reason, user_once, reply,
                )
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
        reason: String,
        user_once: bool,
        reply: oneshot::Sender<PermissionOutcome>,
    ) -> Result<()> {
        if reply.is_closed() {
            return Ok(());
        }
        // user-once 不建议任何持久 / 会话规则（TS suggestedPermissionUpdates = []）。
        let suggested = if user_once {
            vec![]
        } else {
            suggested_rules(tool, input)
        };
        let hook_reason = reason.clone();
        // 项目规则可由 store 持久化时才投放 allowAlways，避免给出无法兑现的授权；会话级授权随会话落库。
        let persistent = self.project_rules_persistent
            || self
                .sessions
                .get(id)
                .is_some_and(|s| s.scoped_permission.is_some());
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
        let mut payload = json!({
            "kind": "permission",
            "toolCallId": call["id"],
            "toolName": tool,
            // 与 TS 一致：summary 是判定原因（原先是自拟的 "Allow {tool}?"）。
            "summary": reason,
            "detail": input,
            "freeText": true,
            "options": if user_once {
                // TS buildPermissionOptions：user-once 只留「允许一次 / 拒绝」。
                options_for(tool, &suggested, persistent)
                    .into_iter()
                    .filter(|option| matches!(option["kind"].as_str(), Some("allowOnce" | "deny")))
                    .collect()
            } else {
                options_for(tool, &suggested, persistent)
            },
        });
        // TS 仅对主会话的确认提供「完全访问」（子代理来源的确认不带该选项）；声明了选项策略的工具
        // （工作流的 Create / Amend / Save）同样不带。
        if s.parent_id.is_none() && !OPTIONS_POLICY_TOOLS.contains(&tool) && !user_once {
            payload["fullAccessOption"] = full_access_option();
        }
        s.pending.push(json!({
            "interactionId": interaction,
            "kind": "permission",
            "anchorRowId": s.rows.iter().find(|r| r["toolCallId"] == call["id"]).map(|r| r["rowId"].clone()).unwrap_or(Value::Null),
            "createdAt": now,
            "payload": payload,
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
        let call_id = call["id"].as_str().unwrap_or_default();
        let p = super::telemetry::Permission {
            phase: "requested",
            call_id,
            tool: Some(tool),
            request_id: Some(&interaction),
            decision: None,
        };
        self.telemetry_permission(id, Some(turn), p);
        if !user_once {
            self.spawn_permission_hook(id, turn, call, input, &interaction, &hook_reason);
        }
        self.publish(id, deltas)?;
        self.persist(id, None).await
    }
}

pub(super) use super::permission_rules::FULL_ACCESS_OPTION_ID;
use super::permission_rules::{
    OPTIONS_POLICY_TOOLS, full_access_option, merge_allow_rules, options_for, suggested_rules,
};

impl Engine {
    /// 「总是允许」落点：群任务写会话级授权（随会话落库，TS persistProjectPermissionUpdates 的
    /// scope=session 分支），否则写项目规则。失败时拒绝这次授权而不是静默放行。
    pub(super) async fn merge_allow_always(&mut self, id: &str, rules: &[Rule]) -> bool {
        let Some(scoped) = self
            .sessions
            .get_mut(id)
            .and_then(|s| s.scoped_permission.as_mut())
        else {
            return self.merge_project_rules(rules).await;
        };
        merge_allow_rules(scoped, rules);
        self.persist(id, None).await.is_ok()
    }

    /// 「总是允许」：把建议规则并入项目规则集并落库；失败时拒绝这次授权而不是静默放行。
    pub(super) async fn merge_project_rules(&mut self, rules: &[Rule]) -> bool {
        if !self.project_rules_persistent {
            return false;
        }
        let mut json = self
            .project_rules_json
            .clone()
            .unwrap_or_else(|| json!({"version": 1}));
        merge_allow_rules(&mut json, rules);
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
