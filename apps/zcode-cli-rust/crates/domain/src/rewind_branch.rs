//! 会话回退后的活跃分支，逐行对齐 TS `@zcode/contracts` 的 `selectActiveConversationBranch`。
//! 由 scripts/generate-zcode-cli-rust-rewind-branch-corpus.mjs 用 TS 实现导出语料校验。
use serde_json::Value;

/// `ids` 为按存储顺序排列的消息 id；`revert` 为 session.revert（无回退时为 Null）。
/// 返回活跃分支上消息在 `ids` 中的下标，保持 TS 的顺序（kept 按 keptMessageIDs 顺序在前）。
pub fn active_branch(ids: &[String], revert: &Value) -> Vec<usize> {
    let all = || (0..ids.len()).collect::<Vec<_>>();
    let Some(target) = revert["targetMessageID"].as_str() else {
        return all();
    };
    let index_of = |id: &str| ids.iter().position(|m| m == id);
    let target_index = index_of(target);
    let kept_ids = revert["keptMessageIDs"].as_array();
    let kept: Vec<usize> = match (kept_ids, target_index) {
        (Some(list), _) => list
            .iter()
            .filter_map(|id| id.as_str().and_then(index_of))
            .collect(),
        (None, Some(t)) => (0..t).collect(),
        (None, None) => all(),
    };
    let with_tail = |from: usize| {
        let mut out = kept.clone();
        out.extend(from..ids.len());
        out
    };
    if let Some(cut) = revert["branchCutAfterMessageID"].as_str() {
        return match index_of(cut) {
            Some(i) => with_tail(i + 1),
            None => kept,
        };
    }
    let created = revert["createdMessageID"].as_str();
    if kept_ids.is_some() {
        return match created.and_then(index_of) {
            Some(i) => with_tail(i),
            None => kept,
        };
    }
    if target_index.is_none() {
        return all();
    }
    match created.and_then(index_of) {
        Some(i) => with_tail(i),
        None => kept,
    }
}
