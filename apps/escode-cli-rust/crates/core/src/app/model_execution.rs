//! 单次执行作用域（modelExecution，docs/specs/rust-offpeak.md 第三期）的 owner 状态。
//!
//! - admission：本轮选择不写入会话模型；凭据按 turn 暂存（只在内存），payload 只留无秘密的标记；
//! - 运行开始：按 (会话, run) 取出成为本轮执行材料；前台子代理在派生时继承父轮材料；
//! - off-peak 账号模型请求鉴权时，由这里直接应答本轮凭据，不向 Host 请求 header；
//! - 过期条目只在 run 不匹配时被忽略，下一轮开始时覆盖。

use super::Engine;
use crate::contract::ModelIdentity;
use crate::domain::model_execution::ModelExecution;
use serde_json::Value;
use std::collections::BTreeMap;

/// payload 中的无秘密标记（本轮为执行作用域；skipMemory/subagents 两个约束）。
pub(super) const MARKER: &str = "_modelExecution";

#[derive(Clone)]
pub(super) struct Live {
    pub run_id: String,
    pub execution: ModelExecution,
    pub selection: ModelIdentity,
}

#[derive(Default)]
pub(super) struct Executions {
    /// turn → 暂存的执行材料（admission 到运行开始）。
    pending: BTreeMap<String, ModelExecution>,
    /// 会话 → 正在运行的执行材料（按 run_id 校验）。
    live: BTreeMap<String, Live>,
    /// 子会话 → 继承自父轮的材料（子会话首轮开始时取用）。
    inherit: BTreeMap<String, (ModelExecution, ModelIdentity)>,
}

impl Engine {
    pub(super) fn stash_execution(&mut self, turn: &str, execution: ModelExecution) {
        self.shell.executions.pending.insert(turn.to_owned(), execution);
    }
    /// 运行开始：本轮有执行标记则取出暂存材料，否则取子会话继承的材料。
    pub(super) fn begin_execution(
        &mut self,
        id: &str,
        run_id: &str,
        turn: &str,
        payload: &Value,
        selection: &ModelIdentity,
    ) -> Option<Live> {
        let executions = &mut self.shell.executions;
        let (execution, selection) = if payload.get(MARKER).is_some() {
            let execution = executions.pending.remove(turn).unwrap_or_else(|| ModelExecution {
                // 重启后暂存凭据已不在内存：保留约束，凭据缺失时 off-peak 模型请求失败（TS ModelRequestAuthMissing）。
                skip_memory: payload[MARKER]["skipMemory"] == true,
                request_auth: None,
                subagents: payload[MARKER]["subagents"] == true,
            });
            (execution, selection.clone())
        } else {
            executions.inherit.remove(id)?
        };
        let live = Live { run_id: run_id.to_owned(), execution, selection };
        executions.live.insert(id.to_owned(), live.clone());
        Some(live)
    }
    pub(super) fn live_execution(&self, id: &str, run_id: &str) -> Option<&Live> {
        self.shell.executions.live.get(id).filter(|live| live.run_id == run_id)
    }
    /// 前台子代理继承父轮的执行模型与凭据（TS subagentModelOverride）。
    pub(super) fn inherit_execution(&mut self, parent: &str, child: &str) {
        let parent_run = self.active.get(parent).map(|a| a.run_id.clone()).unwrap_or_default();
        if let Some(live) = self.live_execution(parent, &parent_run).filter(|l| l.execution.subagents).cloned() {
            self.shell.executions.inherit.insert(child.to_owned(), (live.execution, live.selection));
        }
    }
}
