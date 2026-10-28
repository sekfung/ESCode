//! `TasksIndex` 的任务分组方法（实现见 `crate::task_groups`）。
//!
//! 每个方法在单个写事务内完成：分组写入与任务事实读取共享同一快照，失败整体回滚。

use serde_json::Value;

use super::project_read::{now_ms, transaction};
use super::{TasksIndex, TasksIndexError};
use crate::task_groups::{admit, node_move, repository};

fn wrap<T>(result: anyhow::Result<T>) -> Result<T, TasksIndexError> {
    result.map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
}

impl TasksIndex {
    pub fn task_group_read(&self) -> Result<Value, TasksIndexError> {
        wrap(transaction(&self.connection, false, || {
            admit::read(&self.connection)
        }))
    }

    /// 解析分组入参引用对应的任务事实；不存在的引用直接缺席（由 channel 判可用性）。
    pub fn task_group_resolve_facts(&self, refs: &[Value]) -> Result<Vec<Value>, TasksIndexError> {
        wrap(
            refs.iter()
                .map(|task| crate::task_groups::read_group_task_fact(&self.connection, task))
                .collect::<anyhow::Result<Vec<_>>>()
                .map(|facts| facts.into_iter().flatten().collect()),
        )
    }

    pub fn task_group_save(&self, input: &Value) -> Result<Value, TasksIndexError> {
        wrap(transaction(&self.connection, true, || {
            repository::save(&self.connection, input, now_ms())
        }))
    }

    pub fn task_group_ungroup(&self, id: &str) -> Result<(), TasksIndexError> {
        wrap(transaction(&self.connection, true, || {
            repository::ungroup(&self.connection, id, now_ms())
        }))
    }

    pub fn task_group_move(&self, input: &Value) -> Result<(), TasksIndexError> {
        wrap(transaction(&self.connection, true, || {
            node_move::move_node(&self.connection, input, true, now_ms())
        }))
    }

    /// `admit` 入参已由 channel 解析为任务事实（`tasks` 数组 + 可选 `placement`）。
    pub fn task_group_admit(&self, input: &Value) -> Result<(), TasksIndexError> {
        wrap(transaction(&self.connection, true, || {
            admit::admit(&self.connection, input, now_ms())
        }))
    }
}
