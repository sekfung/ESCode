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
pub const TOOLS: [&str; 11] = [
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
    // v3.15.1：留白补全。
    "FillWorkflowHole",
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
    /// v3.15.1 `mode: onDemand`：工具面推迟到会话首次激活（`/workflow`、run 控制、恢复出有工作流的
    /// 会话）。进程级结论只在开启时有意义（TS 关闭时清掉 mode）；会话按创建/恢复时固化。
    process_on_demand: bool,
    on_demand: BTreeMap<String, bool>,
}

/// TS `DynamicWorkflowMode` 中 onDemand 的判定（缺席 = alwaysOn，供只发布尔的旧 Host 配对）。
pub fn is_on_demand(mode: Option<&str>) -> Option<bool> {
    mode.map(|mode| mode == "onDemand")
}

impl Policy {
    /// 进程级结论。TUI / headless / workflow_child 的「不参与灰度」由调用方表达（不建会话即不追问）。
    pub fn process_enabled(&self) -> bool {
        self.process
    }

    /// `workspace/updateDynamicWorkflowPolicy`：仍在运行的会话先按**旧**结论固化，再翻转进程级结论。
    /// `on_demand` 为 mode 是否 onDemand（缺席即 alwaysOn）；关闭时不保留 mode。
    pub fn update(&mut self, live: &[String], enabled: bool, on_demand: Option<bool>) {
        let (previous, previous_on_demand) = (self.process, self.process_on_demand);
        for id in live {
            self.sessions.entry(id.clone()).or_insert(previous);
            self.on_demand.entry(id.clone()).or_insert(previous_on_demand);
        }
        self.process = enabled;
        self.process_on_demand = enabled && on_demand == Some(true);
    }

    /// 会话创建/恢复：`dynamicWorkflowEnabled === true` 或进程级结论。显式布尔，受信 Host 创建的
    /// 会话不会落进「缺席即保留全部工具」那条豁免。mode 同序取值（参数 → 进程级 → alwaysOn），
    /// 对应 TS `resolveDynamicWorkflowRuntimeGate`。
    pub fn fix(&mut self, id: &str, requested: bool, requested_on_demand: Option<bool>) {
        let enabled = requested || self.process;
        self.sessions.insert(id.to_owned(), enabled);
        let on_demand = enabled && requested_on_demand.unwrap_or(self.process_on_demand);
        self.on_demand.insert(id.to_owned(), on_demand);
    }

    /// 本会话是否 onDemand（未固化时按进程级结论回答）。
    pub fn on_demand(&self, id: &str) -> bool {
        self.on_demand
            .get(id)
            .copied()
            .unwrap_or(self.process_on_demand)
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
