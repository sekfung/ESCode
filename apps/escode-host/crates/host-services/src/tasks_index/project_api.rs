//! `TasksIndex` 的项目方法（实现分别在 project_read/write/misc）。

use super::{TasksIndex, TasksIndexError, project_misc, project_read, project_write};
use serde_json::Value;

impl TasksIndex {
    /// 项目列表（上游 `ProjectRepository.readProjects`）。
    pub fn project_read_projects(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<Value>, TasksIndexError> {
        project_read::read_projects(&self.connection, include_deleted)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    /// 项目/任务树（上游 `ProjectRepository.readTree`）。
    pub fn project_read_tree(
        &self,
        include_deleted: bool,
        include_deleted_tasks: bool,
        include_tasks: bool,
    ) -> Result<Value, TasksIndexError> {
        project_read::read_tree(
            &self.connection,
            include_deleted,
            include_deleted_tasks,
            include_tasks,
        )
        .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_save(&self, input: &Value) -> Result<Value, TasksIndexError> {
        project_write::save_project(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_initialize_root(&self, input: &Value) -> Result<Value, TasksIndexError> {
        project_write::initialize_project_root(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_delete(&self, input: &Value) -> Result<(), TasksIndexError> {
        project_write::delete_project(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_reorder(&self, input: &Value) -> Result<(), TasksIndexError> {
        project_write::reorder_projects(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_move_task(&self, input: &Value) -> Result<(), TasksIndexError> {
        project_write::move_task(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_resolve_directories(&self, task: &Value) -> Result<Value, TasksIndexError> {
        project_write::resolve_session_directories(&self.connection, task)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_reserve_task(&self, input: &Value) -> Result<Value, TasksIndexError> {
        project_write::reserve_created_task(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }

    pub fn project_archive_inactive(&self, input: &Value) -> Result<Vec<Value>, TasksIndexError> {
        project_misc::archive_inactive(&self.connection, input)
            .map_err(|error| TasksIndexError::Rejected(format!("{error:#}")))
    }
}
