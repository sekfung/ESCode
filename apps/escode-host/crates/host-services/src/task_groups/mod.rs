//! 任务分组的读写（对齐上游 `task-groups/repository.ts` 的本地 Host 子集）。
//!
//! 只写组织态（`task_groups` / `task_group_members` / `task_group_view_node_orders` /
//! `task_group_admissions`），不改任务归属或会话事实。已删除的闲时任务域（`offPeak`）不进入模型。

pub(crate) mod admit;
pub(crate) mod node_move;
pub(crate) mod repository;

pub use repository::{group_task_key, read_group_task_fact};

#[cfg(test)]
mod tests;
