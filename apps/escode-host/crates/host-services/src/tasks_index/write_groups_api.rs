//! `TasksIndex` 的分组写入口（薄封装，逻辑见 `write_groups` / `write_groups_apply`）。

use serde_json::Value;

use super::write_groups;
use super::{TasksIndex, TasksIndexError};

impl TasksIndex {
    /// TS `createTaskGroup`（见 [`write_groups`]）。
    pub fn create_task_group(
        &self,
        id: &str,
        title: Option<&str>,
        color: Option<&str>,
        now: i64,
    ) -> Result<Value, TasksIndexError> {
        write_groups::create_task_group(&self.connection, id, title, color, now)
    }

    pub fn rename_task_group(
        &self,
        id: &str,
        title: &str,
        now: i64,
    ) -> Result<Value, TasksIndexError> {
        write_groups::rename_task_group(&self.connection, id, title, now)
    }

    pub fn update_task_group_color(
        &self,
        id: &str,
        color: &str,
        now: i64,
    ) -> Result<Value, TasksIndexError> {
        write_groups::update_task_group_color(&self.connection, id, color, now)
    }

    pub fn delete_task_group(&self, id: &str) -> Result<(), TasksIndexError> {
        write_groups::delete_task_group(&self.connection, id)
    }

    pub fn apply_grouped_task_view_order(
        &self,
        input: &write_groups::OrderInput<'_>,
        now: i64,
    ) -> Result<(), TasksIndexError> {
        write_groups::apply_grouped_task_view_order(&self.connection, input, now)
    }
}
