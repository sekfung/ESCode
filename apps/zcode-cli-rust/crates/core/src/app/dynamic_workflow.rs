//! 动态工作流灰度门（docs/specs/rust-dynamic-workflow.md 第 1 期），对齐 TS
//! `bootstrap/src/zcode-protocol/dynamic-workflow-policy.ts` 与 `registerBuiltInTools` 的
//! `includeDynamicWorkflow` 过滤。读法、固化时机与 fail-closed 缺省由 `domain::dynamic_workflow::Policy`
//! 决定（同 OffPeak）；模块本身只做协议面与工具面接线。

use super::Engine;
use crate::domain::dynamic_workflow;
use anyhow::{Context, Result};
use serde_json::{Value, json};

impl Engine {
    /// `workspace/updateDynamicWorkflowPolicy`（strict `{workspace, enabled}`）：仍在运行的会话
    /// 按旧结论固化，之后创建或恢复的会话按新结论。
    pub(super) fn update_dynamic_workflow_policy(&mut self, p: &Value) -> Result<Value> {
        let enabled = p["enabled"].as_bool().context("Invalid enabled")?;
        let workspace = p
            .get("workspace")
            .filter(|w| w.is_object())
            .context("Invalid workspace")?;
        anyhow::ensure!(
            p.as_object()
                .is_some_and(|o| o.keys().all(|k| k == "workspace" || k == "enabled")),
            "Invalid params"
        );
        let live = self.sessions.keys().cloned().collect::<Vec<_>>();
        self.shell.dynamic_workflow.update(&live, enabled);
        Ok(json!({"workspace": workspace, "enabled": enabled}))
    }

    /// 会话创建/恢复：`dynamicWorkflowEnabled === true` 或进程级结论（fail-closed）。
    pub(super) fn fix_dynamic_workflow(&mut self, id: &str, requested: bool) {
        self.shell.dynamic_workflow.fix(id, requested);
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

/// TS `includeDynamicWorkflow === false`：灰度关闭时下架工作流工具（含只读的 run 内省工具与
/// `ListModels`）。未实现的工具在实现前不注册，所以今天这里没有条目可删；开关与会话的绑定现在
/// 就生效，第 2/6 期把工具注册进来时不再需要动读法。
pub(super) fn retain_visible(definitions: &mut Vec<Value>, facts: &super::context::TurnFacts) {
    definitions.retain(|definition| {
        let name = definition["function"]["name"].as_str().unwrap_or_default();
        facts.dynamic_workflow || !dynamic_workflow::TOOLS.contains(&name)
    });
}
