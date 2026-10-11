//! 动态工作流灰度门（docs/specs/rust-dynamic-workflow.md 第 1 期），对齐 TS
//! `bootstrap/src/zcode-protocol/dynamic-workflow-policy.ts` 与 `registerBuiltInTools` 的
//! `includeDynamicWorkflow` 过滤。读法、固化时机与 fail-closed 缺省由 `domain::dynamic_workflow::Policy`
//! 决定（同 OffPeak）；模块本身只做协议面与工具面接线。

use super::Engine;
use crate::domain::dynamic_workflow;
use anyhow::{Context, Result};
use serde_json::{Value, json};

impl Engine {
    /// `workspace/updateDynamicWorkflowPolicy`（strict `{workspace, enabled, mode?}`）：仍在运行的会话
    /// 按旧结论固化，之后创建或恢复的会话按新结论。`mode`（v3.15.1）缺席即 alwaysOn。
    pub(super) fn update_dynamic_workflow_policy(&mut self, p: &Value) -> Result<Value> {
        let enabled = p["enabled"].as_bool().context("Invalid enabled")?;
        let workspace = p
            .get("workspace")
            .filter(|w| w.is_object())
            .context("Invalid workspace")?;
        anyhow::ensure!(
            p.as_object().is_some_and(|o| o
                .keys()
                .all(|k| k == "workspace" || k == "enabled" || k == "mode")),
            "Invalid params"
        );
        let mode = parse_mode(p.get("mode"))?;
        let live = self.sessions.keys().cloned().collect::<Vec<_>>();
        self.shell
            .dynamic_workflow
            .update(&live, enabled, dynamic_workflow::is_on_demand(mode));
        Ok(json!({"workspace": workspace, "enabled": enabled}))
    }

    /// 会话创建/恢复：`dynamicWorkflowEnabled === true` 或进程级结论（fail-closed）；
    /// `dynamicWorkflowMode` 同序取值（TS resolveDynamicWorkflowRuntimeGate）。
    pub(super) fn fix_dynamic_workflow(&mut self, id: &str, requested: bool, mode: Option<&Value>) {
        let on_demand = parse_mode(mode)
            .ok()
            .and_then(dynamic_workflow::is_on_demand);
        self.shell.dynamic_workflow.fix(id, requested, on_demand);
    }

    /// 本轮模型工具面是否带工作流工具：灰度开启，且 alwaysOn 或本会话已激活（v3.15.1 onDemand）。
    /// 冷恢复出的会话若历史里已有工作流工具调用，按 TS restoreDynamicWorkflowActivationOnResume
    /// 判为已激活并记下（只加不减）。
    pub(super) fn dynamic_workflow_visible(&mut self, id: &str) -> bool {
        if !self.dynamic_workflow_enabled(id) {
            return false;
        }
        if !self.shell.dynamic_workflow.on_demand(id) {
            return true;
        }
        let Some(s) = self.sessions.get_mut(id) else {
            return false;
        };
        if !s.dynamic_workflow_activated && history_shows_dynamic_workflow(&s.messages) {
            s.dynamic_workflow_activated = true;
        }
        s.dynamic_workflow_activated
    }

    /// TS activateDynamicWorkflowTools：激活本会话的工作流工具面（只加不减，随会话下次落库持久化）。
    pub(super) fn activate_dynamic_workflow(&mut self, id: &str) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.dynamic_workflow_activated = true;
        }
    }

    /// 本会话是否注册工作流工具（首次读取时按当时的进程级结论固化）。
    pub(super) fn dynamic_workflow_enabled(&mut self, id: &str) -> bool {
        self.shell.dynamic_workflow.enabled(id)
    }

    /// 固化会话技能目录：动态工作流关闭时去掉内置技能包（TS `collectDynamicWorkflowDisabledSkillPaths`）——
    /// 工作流工具都不在场时再让模型读到「怎么写工作流脚本」只会诱导它调不存在的工具。
    pub(super) fn freeze_skills(
        &mut self,
        id: &str,
        mut catalog: crate::domain::skills::SkillCatalog,
    ) -> crate::domain::skills::SkillCatalog {
        if !self.dynamic_workflow_enabled(id) {
            catalog.skills.retain(|skill| skill.scope != "system");
        }
        catalog
    }
}

/// `mode` 取值域（TS `zcodeDynamicWorkflowModeSchema`）。
fn parse_mode(mode: Option<&Value>) -> Result<Option<&str>> {
    match mode {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(mode)) if matches!(mode.as_str(), "disabled" | "onDemand" | "alwaysOn") => {
            Ok(Some(mode.as_str()))
        }
        Some(_) => anyhow::bail!("Invalid mode"),
    }
}

/// TS `persistedMessagesShowDynamicWorkflow`：历史里有任一工作流工具调用，或一条 `/workflow` 用户输入。
fn history_shows_dynamic_workflow(messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        let workflow_call = message["tool_calls"].as_array().is_some_and(|calls| {
            calls.iter().any(|call| {
                dynamic_workflow::TOOLS.contains(&call["function"]["name"].as_str().unwrap_or(""))
            })
        });
        workflow_call
            || message["role"] == "user"
                && message["content"].as_str().is_some_and(is_workflow_command)
    })
}

/// TS `WORKFLOW_COMMAND_TEXT_PATTERN`：`/^\/workflow(?:\s|$)/i`。
pub(super) fn is_workflow_command(text: &str) -> bool {
    let text = text.trim_start();
    text.get(..9)
        .is_some_and(|head| head.eq_ignore_ascii_case("/workflow"))
        && text[9..].chars().next().is_none_or(char::is_whitespace)
}

/// TS `includeDynamicWorkflow === false`：灰度关闭时下架工作流工具（含只读的 run 内省工具与
/// `ListModels`）。未实现的工具在实现前不注册，所以今天这里没有条目可删；开关与会话的绑定现在
/// 就生效，第 2/6 期把工具注册进来时不再需要动读法。
pub(super) fn retain_visible(definitions: &mut Vec<Value>, facts: &super::context::TurnFacts) {
    definitions.retain(|definition| {
        let name = definition["function"]["name"].as_str().unwrap_or_default();
        facts.dynamic_workflow || !dynamic_workflow::TOOLS.contains(&name)
    });
}
