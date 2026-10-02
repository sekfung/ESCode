//! 动态工作流的共享词汇表（docs/specs/rust-dynamic-workflow.md 第 1 期）：灰度门与工具面名单。
//!
//! 工具在实现前不注册（各期按序推进）；开关现在就与会话绑定，之后各期只需把工具注册进来。

use std::collections::BTreeMap;

/// TS `DYNAMIC_WORKFLOW_TOOL_NAMES`：灰度关闭时不注册的十个工具。
///
/// 关闭的语义是「没有任何办法开始一条工作流」，所以创建、修订、保存、快照实验与四个 run 面
/// 工具一起下架；只读的 run 内省工具也在列，因为关闭态下它们只会指向用户无法再操作的历史。
/// `ListModels` 也在列：它唯一的用途是给一次 run 挑 `subagent_model`。
/// 旧的 `Workflow` 工具（`/expert` 脚本通道）是另一个功能，**不在**名单里。
pub const TOOLS: [&str; 10] = [
    "CreateWorkflow",
    "AmendWorkflow",
    "SaveWorkflow",
    "ListSavedWorkflows",
    "ListModels",
    "EvalWorkflowSnippet",
    "ListWorkflowRuns",
    "GetWorkflowRun",
    "ResumeWorkflowRun",
    "ResolveWorkflowQuestion",
];

/// 灰度门：进程级结论 + 各会话固化值（TS `appRuntimePreferences.dynamicWorkflowEnabled` 与每条
/// record 的 `runtimeConfig.dynamicWorkflowEnabled`）。
///
/// 读法与 OffPeak 同构：会话创建/恢复参数优先，缺席时读进程级结论；翻转只影响之后创建或恢复的
/// 会话，已固化的会话不回收（TS「已活跃 record 的工具面不回收」）。缺省是 fail-closed：受信 Host
/// 必须显式调用 `workspace/updateDynamicWorkflowPolicy` 才开启。
#[derive(Default)]
pub struct Policy {
    process: bool,
    sessions: BTreeMap<String, bool>,
}

impl Policy {
    /// 进程级结论。TUI / headless / workflow_child 的「不参与灰度」由调用方表达（不建会话即不追问）。
    pub fn process_enabled(&self) -> bool {
        self.process
    }

    /// `workspace/updateDynamicWorkflowPolicy`：仍在运行的会话先按**旧**结论固化，再翻转进程级结论。
    pub fn update(&mut self, live: &[String], enabled: bool) {
        let previous = self.process;
        for id in live {
            self.sessions.entry(id.clone()).or_insert(previous);
        }
        self.process = enabled;
    }

    /// 会话创建/恢复：`dynamicWorkflowEnabled === true` 或进程级结论。显式布尔，受信 Host 创建的
    /// 会话不会落进「缺席即保留全部工具」那条豁免。
    pub fn fix(&mut self, id: &str, requested: bool) {
        let enabled = requested || self.process;
        self.sessions.insert(id.to_owned(), enabled);
    }

    /// 只读查询：未固化时按进程级结论回答（不固化）。
    pub fn peek(&self, id: &str) -> bool {
        self.sessions.get(id).copied().unwrap_or(self.process)
    }

    /// 本会话是否注册工作流工具（首次读取时按当时的进程级结论固化）。
    pub fn enabled(&mut self, id: &str) -> bool {
        let process = self.process;
        *self.sessions.entry(id.to_owned()).or_insert(process)
    }
}

#[cfg(test)]
#[path = "dynamic_workflow_tests.rs"]
mod tests;
