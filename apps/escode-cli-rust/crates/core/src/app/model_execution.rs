//! 单次执行作用域（modelExecution）的 owner 状态。
//!
//! - admission：本轮选择不写入会话模型；凭据按 turn 暂存（只在内存），payload 只留无秘密的标记；
//! - 运行开始：按 (会话, run) 取出成为本轮执行材料；
//! - 账号模型请求鉴权时，由这里直接应答本轮凭据，不向 Host 请求 header；
//! - 过期条目只在 run 不匹配时被忽略，下一轮开始时覆盖。

use super::Engine;
use crate::domain::model_execution::ModelExecution;
use serde_json::Value;
use std::collections::BTreeMap;

/// payload 中的无秘密标记（本轮为执行作用域）。
pub(super) const MARKER: &str = "_modelExecution";

#[derive(Clone)]
pub(super) struct Live {
    pub run_id: String,
    pub execution: ModelExecution,
}

#[derive(Default)]
pub(super) struct Executions {
    /// turn → 暂存的执行材料（admission 到运行开始）。
    pending: BTreeMap<String, ModelExecution>,
    /// 会话 → 正在运行的执行材料（按 run_id 校验）。
    live: BTreeMap<String, Live>,
}

impl Engine {
    pub(super) fn stash_execution(&mut self, turn: &str, execution: ModelExecution) {
        self.shell
            .executions
            .pending
            .insert(turn.to_owned(), execution);
    }
    /// 运行开始：本轮有执行标记则取出暂存材料（重启后凭据已不在内存，保留空的执行作用域）。
    pub(super) fn begin_execution(&mut self, id: &str, run_id: &str, turn: &str, payload: &Value) {
        if payload.get(MARKER).is_none() {
            return;
        }
        let executions = &mut self.shell.executions;
        let execution = executions.pending.remove(turn).unwrap_or_default();
        executions.live.insert(
            id.to_owned(),
            Live {
                run_id: run_id.to_owned(),
                execution,
            },
        );
    }
    pub(super) fn live_execution(&self, id: &str, run_id: &str) -> Option<&Live> {
        self.shell
            .executions
            .live
            .get(id)
            .filter(|live| live.run_id == run_id)
    }
}
